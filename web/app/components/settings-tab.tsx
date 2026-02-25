import type { FormEvent, KeyboardEvent } from "react";

import {
  DEFAULT_MONITOR_SHORTCUT_BASE,
  DEFAULT_PROFILE_SHORTCUT_BASE,
} from "@/app/ui";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
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
import { cn } from "@/lib/utils";
import { openExternalUrl, type ReleaseUpdateCheckResult } from "@/tauri";
import type { AppSnapshot } from "@/types";

function shortcutBaseFromKeyEvent(event: KeyboardEvent<HTMLInputElement>): string | null {
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
  const displayValue = value.trim() || defaultValue;

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Tab") {
      return;
    }
    event.preventDefault();

    if (event.key === "Escape" || event.key === "Backspace" || event.key === "Delete") {
      onChange(defaultValue);
      return;
    }

    const base = shortcutBaseFromKeyEvent(event);
    if (base) {
      onChange(base);
    }
  };

  return (
    <label className="grid gap-2 text-sm">
      <span className="font-medium text-foreground">{label}</span>
      <Input
        id={id}
        type="text"
        readOnly
        autoComplete="off"
        value={displayValue}
        onKeyDown={handleKeyDown}
        onPaste={(event) => event.preventDefault()}
        className="sm:max-w-sm font-mono"
      />
      <p className="text-xs text-muted-foreground">{description}</p>
    </label>
  );
}

type SettingsTabProps = {
  loading: boolean;
  snapshot: AppSnapshot | null;
  settingsDirty: boolean;
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
  const startupProfileSelectValue = startupProfileName ?? "__none__";
  const hasSelectedProfileInList =
    startupProfileName == null ||
    snapshot?.profiles.some((profile) => profile.name === startupProfileName);

  return (
    <TabsContent value="settings" className="mt-0">
      {!loading && snapshot ? (
        <main className="grid gap-4">
          <Card>
            <CardHeader className="space-y-1">
              <div className="flex flex-wrap items-center gap-2">
                <CardTitle className="text-base">Settings</CardTitle>
                {settingsDirty ? (
                  <Badge variant="outline">Unsaved changes</Badge>
                ) : null}
              </div>
              <CardDescription>
                Configure Monarch.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form className="grid max-w-xl gap-4" onSubmit={onSettingsSubmit}>
                <label className="grid gap-2 text-sm text-muted-foreground">
                  <span className="font-medium text-foreground">
                    Revert timeout (seconds)
                  </span>
                  <Input
                    type="text"
                    inputMode="numeric"
                    pattern="[0-9]*"
                    autoComplete="off"
                    value={revertTimeoutInput}
                    onChange={(event) => onRevertTimeoutInputChange(event.target.value)}
                    className="w-32"
                  />
                </label>

                <div className="flex items-start gap-3">
                  <Checkbox
                    id="start-with-windows"
                    checked={startWithWindows}
                    onCheckedChange={(checked) => onStartWithWindowsChange(checked === true)}
                    className="mt-0.5"
                  />
                  <label
                    htmlFor="start-with-windows"
                    className="grid gap-1 text-sm leading-snug"
                  >
                    <span className="font-medium text-foreground">
                      Start with Windows (delayed and minimized)
                    </span>
                    <span className="text-muted-foreground">
                      Launch Monarch about 10 seconds after sign-in, hidden to the tray.
                    </span>
                  </label>
                </div>

                <div className="grid gap-2 text-sm">
                  <span className="font-medium text-foreground">
                    Launch profile (optional)
                  </span>
                  <Select
                    value={startupProfileSelectValue}
                    onValueChange={(value) =>
                      onStartupProfileNameChange(value === "__none__" ? null : value)
                    }
                  >
                    <SelectTrigger className="w-full sm:w-80">
                      <SelectValue placeholder="Do not apply a profile" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__none__">Do not apply a profile</SelectItem>
                      {!hasSelectedProfileInList && startupProfileName ? (
                        <SelectItem value={startupProfileName}>
                          {startupProfileName} (missing)
                        </SelectItem>
                      ) : null}
                      {snapshot.profiles.map((profile) => (
                        <SelectItem key={profile.name} value={profile.name}>
                          {profile.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground">
                    Applied automatically whenever Monarch launches.
                  </p>
                </div>

                <div className="grid gap-3 rounded-xl border p-4">
                  <div className="grid gap-1">
                    <span className="font-medium text-foreground">
                      Global Shortcuts
                    </span>
                    <p className="text-xs text-muted-foreground">
                      Works even when Monarch is not focused. Set a modifier base (for example{" "}
                      <span className="font-medium text-foreground">Ctrl+Alt</span>) and Monarch
                      will append each item&apos;s shortcut ID (
                      <span className="font-medium text-foreground">1-9</span>, then{" "}
                      <span className="font-medium text-foreground">0</span> for the 10th item).
                      Use modifier-style bases only (for example{" "}
                      <span className="font-medium text-foreground">Ctrl+Alt</span>), not
                      multi-key chords like{" "}
                      <span className="font-medium text-foreground">Ctrl+M</span>.
                    </p>
                  </div>

                  <div className="flex items-start gap-3 rounded-lg border bg-muted/30 p-3">
                    <Checkbox
                      id="enable-global-shortcuts"
                      checked={globalShortcutsEnabled}
                      onCheckedChange={(checked) =>
                        onGlobalShortcutsEnabledChange(checked === true)
                      }
                      className="mt-0.5"
                    />
                    <label
                      htmlFor="enable-global-shortcuts"
                      className="grid gap-1 text-sm leading-snug"
                    >
                      <span className="font-medium text-foreground">
                        Enable global shortcuts
                      </span>
                      <span className="text-muted-foreground">
                        When disabled, Monarch will not register any global hotkeys.
                      </span>
                    </label>
                  </div>

                  <div className="grid gap-2">
                    <ShortcutBaseField
                      id="profile-shortcut-base"
                      label="Profile Shortcut Base"
                      value={profileShortcutBase}
                      defaultValue={DEFAULT_PROFILE_SHORTCUT_BASE}
                      onChange={onProfileShortcutBaseChange}
                      description="Click and press your preferred modifier combo (e.g. press Ctrl+Shift+1 to record Ctrl+Shift). Profiles use IDs shown in the Profiles tab."
                    />
                  </div>

                  <div className="grid gap-2">
                    <ShortcutBaseField
                      id="monitor-shortcut-base"
                      label="Monitor Toggle Shortcut Base"
                      value={displayShortcutBase}
                      defaultValue={DEFAULT_MONITOR_SHORTCUT_BASE}
                      onChange={onDisplayShortcutBaseChange}
                      description="Click and press your preferred modifier combo (e.g. press Ctrl+Alt+1 to record Ctrl+Alt). Monitor IDs and full shortcuts are shown on the Main tab."
                    />
                  </div>
                </div>

                <p
                  className={cn(
                    "text-xs",
                    settingsValidationMessage ? "text-destructive" : "text-muted-foreground",
                  )}
                >
                  {settingsValidationMessage ??
                    "Used for automatic rollback after applying a layout change."}
                </p>
                <div className="flex flex-wrap gap-2">
                  <Button type="submit" disabled={!canSubmitSettings}>
                    Save Settings
                  </Button>
                </div>
              </form>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="space-y-1">
              <CardTitle className="text-base">Updates</CardTitle>
              <CardDescription>
                Check GitHub Releases for a newer Monarch version.
              </CardDescription>
            </CardHeader>
            <CardContent className="grid gap-3">
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant="outline"
                  disabled={checkingUpdates}
                  onClick={onCheckForUpdates}
                >
                  {checkingUpdates ? "Checking..." : "Check for Updates"}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  onClick={() => {
                    void openExternalUrl(releasesUrl);
                  }}
                >
                  Open Releases
                </Button>
              </div>

              {updateCheckResult ? (
                <div className="rounded-xl border p-3 text-sm">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-muted-foreground">Current:</span>
                    <span className="font-medium text-foreground">
                      v{updateCheckResult.currentVersion}
                    </span>
                    <span className="text-muted-foreground">Latest:</span>
                    <span className="font-medium text-foreground">
                      {updateCheckResult.latestTag}
                    </span>
                    <Badge
                      variant={updateCheckResult.updateAvailable ? "default" : "secondary"}
                    >
                      {updateCheckResult.updateAvailable
                        ? "Update available"
                        : "Up to date"}
                    </Badge>
                  </div>
                  {updateCheckResult.updateAvailable ? (
                    <p className="mt-2 text-muted-foreground">
                      A newer version is available on GitHub Releases.
                    </p>
                  ) : (
                    <p className="mt-2 text-muted-foreground">
                      You are using the latest published release.
                    </p>
                  )}
                </div>
              ) : (
                <p className="text-sm text-muted-foreground">
                  No update check yet.
                </p>
              )}

              {updateCheckError ? (
                <p className="text-sm text-destructive">
                  Could not check for updates: {updateCheckError}
                </p>
              ) : null}
            </CardContent>
          </Card>
        </main>
      ) : null}
    </TabsContent>
  );
}
