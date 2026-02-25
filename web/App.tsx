import { useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { toast } from "sonner";

import { Card, CardContent } from "@/components/ui/card";
import { Tabs } from "@/components/ui/tabs";
import { AppHeader } from "@/app/components/app-header";
import {
  DeleteProfileDialog,
  DisplayToggleDialog,
  PendingConfirmationDialog,
} from "@/app/components/dialogs";
import { MainTab } from "@/app/components/main-tab";
import { ProfilesTab } from "@/app/components/profiles-tab";
import { SettingsTab } from "@/app/components/settings-tab";
import {
  REPO_URL,
  isView,
  type PendingDisplayToggle,
  type View,
} from "@/app/ui";
import { capitalizeToastError, formatMs } from "@/app/utils";

import {
  applyLayout,
  applyProfile,
  checkGithubReleaseUpdate,
  confirmCurrentLayout,
  deleteProfile,
  getSnapshot,
  listenMonarchEvent,
  restoreLastLayout,
  rollbackPending,
  saveProfile,
  toggleDisplay,
  updateSettings,
  type ReleaseUpdateCheckResult,
} from "./tauri";
import type {
  AppSettings,
  AppSnapshot,
  DisplayInfo,
} from "./types";

function App() {
  const [view, setView] = useState<View>("main");
  const [snapshot, setSnapshot] = useState<AppSnapshot | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [pendingLayoutDecisionBusy, setPendingLayoutDecisionBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newProfileName, setNewProfileName] = useState("");
  const [revertTimeoutInput, setRevertTimeoutInput] = useState("10");
  const [startWithWindowsEnabled, setStartWithWindowsEnabled] = useState(false);
  const [startupProfileName, setStartupProfileName] = useState<string | null>(null);
  const [pendingDisplayToggle, setPendingDisplayToggle] =
    useState<PendingDisplayToggle | null>(null);
  const [pendingProfileDelete, setPendingProfileDelete] = useState<string | null>(null);
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [updateCheckResult, setUpdateCheckResult] = useState<ReleaseUpdateCheckResult | null>(null);
  const [updateCheckError, setUpdateCheckError] = useState<string | null>(null);
  const refreshInFlight = useRef<Promise<boolean> | null>(null);
  const refreshQueued = useRef(false);
  const settingsDirtyRef = useRef(false);
  const hasPendingConfirmation = Boolean(snapshot?.pending_confirmation);

  async function refreshStateOnce(): Promise<boolean> {
    try {
      const next = await getSnapshot();
      setSnapshot(next);
      if (!settingsDirtyRef.current) {
        setRevertTimeoutInput(String(next.settings.revert_timeout_secs));
        setStartWithWindowsEnabled(next.settings.start_with_windows);
        setStartupProfileName(next.settings.startup_profile_name);
      }
      setError(null);
      return true;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return false;
    } finally {
      setLoading(false);
    }
  }

  async function refreshState(): Promise<boolean> {
    if (refreshInFlight.current) {
      refreshQueued.current = true;
      return refreshInFlight.current;
    }

    const task = (async () => {
      let ok = await refreshStateOnce();
      while (refreshQueued.current) {
        refreshQueued.current = false;
        ok = await refreshStateOnce();
      }
      return ok;
    })();

    refreshInFlight.current = task;
    try {
      return await task;
    } finally {
      refreshInFlight.current = null;
    }
  }

  async function runAction(
    action: () => Promise<void>,
    successNotice?: string,
    refresh = true,
  ): Promise<boolean> {
    let succeeded = false;
    setBusy(true);
    setError(null);
    try {
      await action();
      succeeded = true;
      if (successNotice) {
        toast.success(successNotice);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      toast.error(capitalizeToastError(message));
      return false;
    }
    finally {
      setBusy(false);
    }

    if (succeeded && refresh) {
      void refreshState();
    }

    return succeeded;
  }

  function runPendingLayoutDecision(
    action: () => Promise<void>,
  ): void {
    if (pendingLayoutDecisionBusy) {
      return;
    }

    setPendingLayoutDecisionBusy(true);
    setError(null);
    setSnapshot((current) =>
      current ? { ...current, pending_confirmation: null } : current,
    );

    void (async () => {
      try {
        await action();
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
        toast.error(capitalizeToastError(message));
      } finally {
        setPendingLayoutDecisionBusy(false);
        void refreshState();
      }
    })();
  }

  useEffect(() => {
    void refreshState();

    let unlistenState: (() => void) | undefined;
    let unlistenConfirm: (() => void) | undefined;

    void listenMonarchEvent("monarch://state-changed", () => {
      void refreshState();
    }).then((dispose) => {
      unlistenState = dispose;
    });

    void listenMonarchEvent<"monarch://confirmation">("monarch://confirmation", (event) => {
      const payload = event.payload;

      if (payload.kind === "applied") {
        toast("Layout applied", {
          description: `Confirm within ${formatMs(payload.timeout_ms)} or it will roll back.`,
        });
      }

      if (payload.kind === "confirmed") {
        toast.success("Layout confirmed.");
      }

      if (payload.kind === "reverted") {
        toast("Layout reverted", {
          description:
            payload.reason === "timeout"
              ? "The rollback timer expired."
              : "The pending layout was reverted.",
        });
      }

      void refreshState();
    }).then((dispose) => {
      unlistenConfirm = dispose;
    });

    const handleVisibilityOrFocus = () => {
      if (document.visibilityState === "visible") {
        void refreshState();
      }
    };

    window.addEventListener("focus", handleVisibilityOrFocus);
    document.addEventListener("visibilitychange", handleVisibilityOrFocus);

    return () => {
      window.removeEventListener("focus", handleVisibilityOrFocus);
      document.removeEventListener("visibilitychange", handleVisibilityOrFocus);
      unlistenState?.();
      unlistenConfirm?.();
    };
  }, []);

  useEffect(() => {
    const intervalMs = hasPendingConfirmation ? 1000 : 4000;
    const interval = window.setInterval(() => {
      if (document.visibilityState === "visible") {
        void refreshState();
      }
    }, intervalMs);

    return () => {
      window.clearInterval(interval);
    };
  }, [hasPendingConfirmation]);

  const activeDisplays = useMemo(
    () => snapshot?.displays.filter((display) => display.is_active) ?? [],
    [snapshot],
  );

  const rawRevertTimeout = revertTimeoutInput.trim();
  const revertTimeoutIsWholeNumber = /^\d+$/.test(rawRevertTimeout);
  const parsedRevertTimeout = revertTimeoutIsWholeNumber ? Number(rawRevertTimeout) : NaN;
  const revertTimeoutInRange =
    revertTimeoutIsWholeNumber && parsedRevertTimeout >= 1 && parsedRevertTimeout <= 60;

  const settingsDirty = useMemo(() => {
    if (!snapshot) {
      return false;
    }

    const timeoutDirty = revertTimeoutIsWholeNumber
      ? parsedRevertTimeout !== snapshot.settings.revert_timeout_secs
      : rawRevertTimeout !== String(snapshot.settings.revert_timeout_secs);

    return (
      timeoutDirty ||
      startWithWindowsEnabled !== snapshot.settings.start_with_windows ||
      startupProfileName !== snapshot.settings.startup_profile_name
    );
  }, [
    parsedRevertTimeout,
    rawRevertTimeout,
    revertTimeoutIsWholeNumber,
    snapshot,
    startWithWindowsEnabled,
    startupProfileName,
  ]);

  useEffect(() => {
    settingsDirtyRef.current = settingsDirty;
  }, [settingsDirty]);

  const actionBusy = busy || pendingLayoutDecisionBusy;
  const canSubmitSettings = Boolean(snapshot) && !actionBusy && settingsDirty && revertTimeoutInRange;

  const settingsValidationMessage =
    rawRevertTimeout.length === 0
      ? "Enter a whole number between 1 and 60."
      : revertTimeoutInRange
        ? null
        : "Revert timeout must be a whole number between 1 and 60.";

  async function handleConfirmDisplayToggle() {
    if (!pendingDisplayToggle) {
      return;
    }

    if (hasPendingConfirmation) {
      toast("Resolve pending confirmation first", {
        description: "Confirm or revert the current layout change before toggling another display.",
      });
      return;
    }

    const target = pendingDisplayToggle;
    setPendingDisplayToggle(null);

    await runAction(
      () => toggleDisplay(target.idKey),
      target.currentlyActive ? "Display detached" : "Display attached",
    );
  }

  async function handleMakePrimaryDisplay(display: DisplayInfo) {
    if (!snapshot) {
      return;
    }

    if (!display.is_active || display.is_primary) {
      return;
    }

    if (hasPendingConfirmation) {
      toast("Resolve pending confirmation first", {
        description: "Confirm or revert the current layout change before selecting a new primary display.",
      });
      return;
    }

    let foundTarget = false;
    const nextLayout = {
      outputs: snapshot.layout.outputs.map((output) => {
        if (output.display_key === display.id_key) {
          foundTarget = true;
          return {
            ...output,
            primary: output.enabled,
          };
        }
        return {
          ...output,
          primary: false,
        };
      }),
    };

    if (!foundTarget) {
      const message = "Could not find display in current layout.";
      setError(message);
      toast.error(message);
      return;
    }

    await runAction(
      () => applyLayout(nextLayout),
      `${display.friendly_name} set as primary`,
    );
  }

  function handleConfirmPendingLayout() {
    runPendingLayoutDecision(confirmCurrentLayout);
  }

  function handleRevertPendingLayout() {
    runPendingLayoutDecision(rollbackPending);
  }

  async function handleConfirmProfileDelete() {
    if (!pendingProfileDelete) {
      return;
    }

    if (hasPendingConfirmation) {
      toast("Resolve pending confirmation first", {
        description: "Profile changes are locked while a layout confirmation is pending.",
      });
      return;
    }

    const profileName = pendingProfileDelete;
    setPendingProfileDelete(null);
    await runAction(() => deleteProfile(profileName), "Profile deleted");
  }

  async function handleCheckForUpdates() {
    setCheckingUpdates(true);
    setUpdateCheckError(null);
    try {
      const result = await checkGithubReleaseUpdate();
      setUpdateCheckResult(result);
      if (result.updateAvailable) {
        toast("Update available", {
          description: `${result.latestTag} is available on GitHub Releases.`,
        });
      } else {
        toast.success("You are on the latest version.");
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setUpdateCheckError(message);
      toast.error(capitalizeToastError(message));
    } finally {
      setCheckingUpdates(false);
    }
  }

  async function handleSaveCurrentLayout() {
    await runAction(
      async () => {
        await saveProfile(newProfileName.trim());
        setNewProfileName("");
      },
      "Profile saved",
    );
  }

  function handleRevertTimeoutInputChange(value: string) {
    setError(null);
    settingsDirtyRef.current = true;
    setRevertTimeoutInput(value);
  }

  function handleStartWithWindowsChange(checked: boolean) {
    setError(null);
    settingsDirtyRef.current = true;
    setStartWithWindowsEnabled(checked);
  }

  function handleStartupProfileNameChange(value: string | null) {
    setError(null);
    settingsDirtyRef.current = true;
    setStartupProfileName(value);
  }

  function handleSettingsSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!revertTimeoutInRange) {
      const message = "Revert timeout must be a whole number between 1 and 60.";
      setError(message);
      toast.error(capitalizeToastError(message));
      return;
    }

    if (!snapshot || !settingsDirty) {
      return;
    }

    const nextSettings: AppSettings = {
      ...snapshot.settings,
      revert_timeout_secs: parsedRevertTimeout,
      start_with_windows: startWithWindowsEnabled,
      startup_profile_name: startupProfileName,
    };
    const normalizedRevertTimeout = String(parsedRevertTimeout);
    void runAction(async () => {
      await updateSettings(nextSettings);
      settingsDirtyRef.current = false;
      setRevertTimeoutInput(normalizedRevertTimeout);
      setStartWithWindowsEnabled(nextSettings.start_with_windows);
      setStartupProfileName(nextSettings.startup_profile_name);
    }, "Settings updated");
  }

  return (
    <div className="min-h-screen">
      <Tabs
        value={view}
        onValueChange={(value) => {
          if (isView(value)) {
            setView(value);
          }
        }}
        className="mx-auto flex w-full max-w-7xl flex-col gap-4 p-4 pb-8 sm:p-6"
      >
        <AppHeader />

        {loading ? (
          <Card>
            <CardContent className="p-6 text-sm text-muted-foreground">
              Loading display topology...
            </CardContent>
          </Card>
        ) : null}

        <MainTab
          loading={loading}
          snapshot={snapshot}
          activeDisplayCount={activeDisplays.length}
          actionBusy={actionBusy}
          hasPendingConfirmation={hasPendingConfirmation}
          onRestoreLastLayout={() => {
            void runAction(restoreLastLayout, "Restored last layout");
          }}
          onMakePrimaryRequest={(selected) => {
            void handleMakePrimaryDisplay(selected);
          }}
          onToggleRequest={(selected) =>
            setPendingDisplayToggle({
              idKey: selected.id_key,
              friendlyName: selected.friendly_name,
              currentlyActive: selected.is_active,
            })
          }
        />

        <ProfilesTab
          loading={loading}
          snapshot={snapshot}
          actionBusy={actionBusy}
          hasPendingConfirmation={hasPendingConfirmation}
          newProfileName={newProfileName}
          onNewProfileNameChange={setNewProfileName}
          onSaveCurrentLayout={() => {
            void handleSaveCurrentLayout();
          }}
          onApplyProfile={(name) => {
            void runAction(() => applyProfile(name), "Profile applied");
          }}
          onDeleteProfileRequest={setPendingProfileDelete}
        />

        <SettingsTab
          loading={loading}
          snapshot={snapshot}
          settingsDirty={settingsDirty}
          revertTimeoutInput={revertTimeoutInput}
          startWithWindows={startWithWindowsEnabled}
          startupProfileName={startupProfileName}
          settingsValidationMessage={settingsValidationMessage}
          canSubmitSettings={canSubmitSettings}
          onSettingsSubmit={handleSettingsSubmit}
          onRevertTimeoutInputChange={handleRevertTimeoutInputChange}
          onStartWithWindowsChange={handleStartWithWindowsChange}
          onStartupProfileNameChange={handleStartupProfileNameChange}
          checkingUpdates={checkingUpdates}
          updateCheckResult={updateCheckResult}
          updateCheckError={updateCheckError}
          onCheckForUpdates={() => {
            void handleCheckForUpdates();
          }}
          releasesUrl={`${REPO_URL}/releases`}
        />

        <PendingConfirmationDialog
          pendingConfirmation={snapshot?.pending_confirmation ?? null}
          busy={pendingLayoutDecisionBusy}
          onRevert={handleRevertPendingLayout}
          onConfirm={handleConfirmPendingLayout}
        />

        <DisplayToggleDialog
          pendingDisplayToggle={pendingDisplayToggle}
          busy={actionBusy}
          onOpenChange={(open) => {
            if (!open) {
              setPendingDisplayToggle(null);
            }
          }}
          onConfirm={() => {
            void handleConfirmDisplayToggle();
          }}
        />

        <DeleteProfileDialog
          pendingProfileDelete={pendingProfileDelete}
          busy={actionBusy}
          onOpenChange={(open) => {
            if (!open) {
              setPendingProfileDelete(null);
            }
          }}
          onConfirm={() => {
            void handleConfirmProfileDelete();
          }}
        />
      </Tabs>
    </div>
  );
}

export default App;
