import type { FormEvent, KeyboardEvent } from "react";
import { Download, ExternalLink, Keyboard, Settings2 } from "lucide-react";

import {
  DEFAULT_MONITOR_SHORTCUT_BASE,
  DEFAULT_PROFILE_SHORTCUT_BASE,
} from "@/app/ui";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { TabsContent } from "@/components/ui/tabs";
import { openExternalUrl, type ReleaseUpdateCheckResult } from "@/tauri";
import type { AppSnapshot } from "@/types";

const NO_STARTUP_PROFILE_VALUE = "__none__";

function shortcutBaseFromKeyEvent(
  event: KeyboardEvent<HTMLInputElement>,
): string | null {
  const parts: string[] = [];
  if (event.ctrlKey) {
    parts.push("Ctrl");
  }
  if (event.altKey) {
    parts.push("Alt");
  }
  if (event.shiftKey) {
    parts.push("Shift");
  }
  if (event.metaKey) {
    parts.push("Super");
  }
  return parts.length > 0 ? parts.join("+") : null;
}

type ShortcutBaseFieldProps = {
  id: string;
  label: string;
  description: string;
  value: string;
  defaultValue: string;
  onChange: (value: string) => void;
};

function ShortcutBaseField({
  id,
  label,
  description,
  value,
  defaultValue,
  onChange,
}: ShortcutBaseFieldProps) {
  const displayValue = value.trim() || "Custom shortcuts";

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Tab") {
      return;
    }
    event.preventDefault();

    if (
      event.key === "Escape" ||
      event.key === "Backspace" ||
      event.key === "Delete"
    ) {
      onChange(defaultValue);
      return;
    }

    const base = shortcutBaseFromKeyEvent(event);
    if (base) {
      onChange(base);
    }
  };

  return (
    <div className="grid min-w-0 gap-1.5 text-sm">
      <label htmlFor={id} className="field-label">
        {label}
      </label>
      <Input
        id={id}
        aria-describedby={`${id}-hint`}
        type="text"
        readOnly
        autoComplete="off"
        value={displayValue}
        onKeyDown={handleKeyDown}
        onPaste={(event) => event.preventDefault()}
        className="font-mono"
      />
      <p id={`${id}-hint`} className="text-xs text-muted-foreground">
        {description}
      </p>
    </div>
  );
}

type SettingsTabProps = {
  loading: boolean;
  snapshot: AppSnapshot | null;
  settingsDirty: boolean;
  disabled: boolean;
  onDiscardSettings: () => void;
  revertTimeoutInput: string;
  startWithWindows: boolean;
  startupProfileName: string | null;
  globalShortcutsEnabled: boolean;
  profileShortcutBase: string;
  displayShortcutBase: string;
  settingsValidationMessage: string | null;
  canSubmitSettings: boolean;
  onSettingsSubmit: (event: FormEvent<HTMLFormElement>) => void;
  onRevertTimeoutInputChange: (value: string) => void;
  onStartWithWindowsChange: (checked: boolean) => void;
  onStartupProfileNameChange: (value: string | null) => void;
  onGlobalShortcutsEnabledChange: (checked: boolean) => void;
  onProfileShortcutBaseChange: (value: string) => void;
  onDisplayShortcutBaseChange: (value: string) => void;
  checkingUpdates: boolean;
  updateCheckResult: ReleaseUpdateCheckResult | null;
  updateCheckError: string | null;
  onCheckForUpdates: () => void;
  releasesUrl: string;
};

export function SettingsTab({
  loading,
  snapshot,
  settingsDirty,
  disabled,
  onDiscardSettings,
  revertTimeoutInput,
  startWithWindows,
  startupProfileName,
  globalShortcutsEnabled,
  profileShortcutBase,
  displayShortcutBase,
  settingsValidationMessage,
  canSubmitSettings,
  onSettingsSubmit,
  onRevertTimeoutInputChange,
  onStartWithWindowsChange,
  onStartupProfileNameChange,
  onGlobalShortcutsEnabledChange,
  onProfileShortcutBaseChange,
  onDisplayShortcutBaseChange,
  checkingUpdates,
  updateCheckResult,
  updateCheckError,
  onCheckForUpdates,
  releasesUrl,
}: SettingsTabProps) {
  const startupProfileSelectValue =
    startupProfileName ?? NO_STARTUP_PROFILE_VALUE;
  const selectedProfileExists =
    startupProfileName == null ||
    snapshot?.profiles.some((profile) => profile.name === startupProfileName);

  return (
    <TabsContent value="settings" className="mt-0">
      {!loading && snapshot ? (
        <main className="grid items-start gap-4 min-[900px]:grid-cols-[minmax(0,1fr)_340px]">
          <Card>
            <CardHeader className="min-h-12 flex-row items-center justify-between py-2">
              <CardTitle className="flex items-center gap-2">
                <Settings2
                  className="size-4 text-muted-foreground"
                  aria-hidden="true"
                />
                Preferences
              </CardTitle>
              {settingsDirty && (
                <Badge variant="outline">Unsaved changes</Badge>
              )}
            </CardHeader>
            <form onSubmit={onSettingsSubmit}>
              <fieldset disabled={disabled} className="divide-y">
                <section className="grid gap-5 p-4 sm:p-5">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <label htmlFor="revert-timeout" className="grid gap-1">
                      <span className="text-sm font-medium">
                        Confirmation timeout
                      </span>
                      <span className="text-xs text-muted-foreground">
                        Revert unconfirmed display changes after 1–60 seconds.
                      </span>
                    </label>
                    <div className="flex items-center gap-2">
                      <Input
                        id="revert-timeout"
                        aria-label="Revert timeout (seconds)"
                        type="text"
                        inputMode="numeric"
                        pattern="[0-9]*"
                        autoComplete="off"
                        value={revertTimeoutInput}
                        onChange={(event) =>
                          onRevertTimeoutInputChange(event.target.value)
                        }
                        className="w-20 font-mono"
                      />
                      <span className="text-xs text-muted-foreground">
                        seconds
                      </span>
                    </div>
                  </div>
                  <div className="flex items-start gap-3">
                    <Checkbox
                      id="start-with-windows"
                      aria-labelledby="start-with-windows-label"
                      aria-describedby="start-with-windows-hint"
                      checked={startWithWindows}
                      onCheckedChange={(checked) =>
                        onStartWithWindowsChange(checked === true)
                      }
                      className="mt-0.5"
                    />
                    <label
                      htmlFor="start-with-windows"
                      className="grid gap-1 text-sm"
                    >
                      <span
                        id="start-with-windows-label"
                        className="font-medium"
                      >
                        Start with Windows
                      </span>
                      <span
                        id="start-with-windows-hint"
                        className="text-xs text-muted-foreground"
                      >
                        Launch in the tray about 10 seconds after sign-in.
                      </span>
                    </label>
                  </div>
                  <div className="grid gap-1.5">
                    <label htmlFor="startup-profile" className="field-label">
                      Launch profile (optional)
                    </label>
                    <Select
                      value={startupProfileSelectValue}
                      disabled={disabled}
                      onValueChange={(value) =>
                        onStartupProfileNameChange(
                          value === NO_STARTUP_PROFILE_VALUE ? null : value,
                        )
                      }
                    >
                      <SelectTrigger id="startup-profile">
                        <SelectValue placeholder="Do not apply a profile" />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value={NO_STARTUP_PROFILE_VALUE}>
                          Do not apply a profile
                        </SelectItem>
                        {!selectedProfileExists && startupProfileName && (
                          <SelectItem value={startupProfileName}>
                            {startupProfileName} (missing)
                          </SelectItem>
                        )}
                        {snapshot.profiles.map((profile) => (
                          <SelectItem key={profile.name} value={profile.name}>
                            {profile.name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <p className="text-xs text-muted-foreground">
                      Applied whenever Monarch launches.
                    </p>
                  </div>
                </section>
                <section className="grid gap-4 p-4 sm:p-5">
                  <h3 className="flex items-center gap-2 text-sm font-medium">
                    <Keyboard
                      className="size-4 text-muted-foreground"
                      aria-hidden="true"
                    />
                    Keyboard shortcuts
                  </h3>
                  <div className="flex items-center gap-3">
                    <Checkbox
                      id="enable-global-shortcuts"
                      checked={globalShortcutsEnabled}
                      onCheckedChange={(checked) =>
                        onGlobalShortcutsEnabledChange(checked === true)
                      }
                    />
                    <label
                      htmlFor="enable-global-shortcuts"
                      className="text-sm"
                    >
                      Enable global shortcuts
                    </label>
                  </div>
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    Click a field and press your modifier keys. Monarch adds
                    each profile or monitor number (1–9, then 0). Backspace
                    restores the default. Existing custom shortcuts stay until
                    you record a new base.
                  </p>
                  <div className="grid gap-4 sm:grid-cols-2">
                    <ShortcutBaseField
                      id="profile-shortcut-base"
                      label="Profile shortcut base"
                      value={profileShortcutBase}
                      defaultValue={DEFAULT_PROFILE_SHORTCUT_BASE}
                      onChange={onProfileShortcutBaseChange}
                      description="Example: Ctrl + Shift + 1 applies the first profile."
                    />
                    <ShortcutBaseField
                      id="monitor-shortcut-base"
                      label="Monitor shortcut base"
                      value={displayShortcutBase}
                      defaultValue={DEFAULT_MONITOR_SHORTCUT_BASE}
                      onChange={onDisplayShortcutBaseChange}
                      description="Example: Ctrl + Alt + 1 toggles monitor 1."
                    />
                  </div>
                </section>
              </fieldset>
              <div className="grid gap-3 border-t bg-muted/10 px-4 py-4 sm:px-5">
                {settingsValidationMessage && (
                  <p role="alert" className="text-xs text-danger-text">
                    {settingsValidationMessage}
                  </p>
                )}
                <div className="flex flex-wrap justify-end gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    disabled={!settingsDirty || disabled}
                    onClick={onDiscardSettings}
                  >
                    Discard changes
                  </Button>
                  <Button
                    type="submit"
                    disabled={!canSubmitSettings || disabled}
                  >
                    Save Settings
                  </Button>
                </div>
              </div>
            </form>
          </Card>
          <Card>
            <CardHeader className="min-h-12 flex-row items-center py-2">
              <CardTitle className="flex items-center gap-2">
                <Download
                  className="size-4 text-muted-foreground"
                  aria-hidden="true"
                />
                Updates
              </CardTitle>
            </CardHeader>
            <CardContent className="grid gap-4">
              <p className="text-xs leading-relaxed text-muted-foreground">
                Check GitHub Releases for a newer version of Monarch.
              </p>
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  variant="outline"
                  disabled={checkingUpdates}
                  onClick={onCheckForUpdates}
                >
                  {checkingUpdates ? "Checking…" : "Check for Updates"}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    void openExternalUrl(releasesUrl);
                  }}
                >
                  Releases
                  <ExternalLink aria-hidden="true" />
                </Button>
              </div>
              {updateCheckResult && (
                <div className="space-y-2 border-t pt-4 text-xs">
                  <div className="flex justify-between gap-2">
                    <span className="text-muted-foreground">Installed</span>
                    <span className="font-mono">
                      v{updateCheckResult.currentVersion}
                    </span>
                  </div>
                  <div className="flex justify-between gap-2">
                    <span className="text-muted-foreground">
                      Latest release
                    </span>
                    <span className="font-mono">
                      {updateCheckResult.latestTag}
                    </span>
                  </div>
                  <p>
                    {updateCheckResult.updateAvailable
                      ? "A newer version is available on GitHub."
                      : "You’re up to date."}
                  </p>
                </div>
              )}
              {updateCheckError && (
                <p role="alert" className="text-xs text-danger-text">
                  Could not check for updates: {updateCheckError}
                </p>
              )}
            </CardContent>
          </Card>
        </main>
      ) : null}
    </TabsContent>
  );
}
