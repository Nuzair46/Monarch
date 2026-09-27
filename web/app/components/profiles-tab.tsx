import { AudioOutputSelect, ProfileAudio } from "./audio-output";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { TabsContent } from "@/components/ui/tabs";
import { indexedShortcutLabel } from "@/app/utils";
import type { AppSnapshot } from "@/types";

type ProfilesTabProps = {
  loading: boolean;
  snapshot: AppSnapshot | null;
  actionBusy: boolean;
  hasPendingConfirmation: boolean;
  shortcutsEnabled: boolean;
  profileShortcutBase: string | null;
  newProfileName: string;
  newProfileAudio: string | null;
  onNewProfileAudioChange: (id: string | null) => void;
  onSaveProfileAudio: (name: string, id: string | null) => void;
  onNewProfileNameChange: (value: string) => void;
  onSaveCurrentLayout: () => void;
  onApplyProfile: (name: string) => void;
  onDeleteProfileRequest: (name: string) => void;
};

export function ProfilesTab({
  loading,
  snapshot,
  actionBusy,
  hasPendingConfirmation,
  shortcutsEnabled,
  profileShortcutBase,
  newProfileName,
  newProfileAudio,
  onNewProfileAudioChange,
  onSaveProfileAudio,
  onNewProfileNameChange,
  onSaveCurrentLayout,
  onApplyProfile,
  onDeleteProfileRequest,
}: ProfilesTabProps) {
  return (
    <TabsContent value="profiles" className="mt-0">
      {!loading && snapshot ? (
        <main className="grid gap-4">
          <Card>
            <CardHeader className="gap-4">
              <div className="space-y-1">
                <CardTitle className="text-base">Profiles</CardTitle>
                <CardDescription>
                  Save a layout with an optional audio output. Saving changes a
                  profile; Apply switches your displays and audio.
                </CardDescription>
              </div>

              <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                <label className="grid gap-1 text-sm">
                  Profile name
                  <Input
                    type="text"
                    placeholder="Profile name"
                    value={newProfileName}
                    onChange={(event) =>
                      onNewProfileNameChange(event.target.value)
                    }
                    className="sm:max-w-sm"
                  />
                </label>
                <div className="min-w-0 sm:w-80">
                  <AudioOutputSelect
                    label="Audio output for new profile"
                    audio={snapshot.audio}
                    value={newProfileAudio}
                    onChange={onNewProfileAudioChange}
                    disabled={actionBusy || hasPendingConfirmation}
                  />
                </div>
                <Button
                  type="button"
                  disabled={
                    actionBusy ||
                    !newProfileName.trim() ||
                    hasPendingConfirmation
                  }
                  onClick={onSaveCurrentLayout}
                >
                  Save Current Layout
                </Button>
              </div>
            </CardHeader>

            <CardContent className="grid gap-3">
              {snapshot.audio.unavailable_reason && (
                <p className="text-sm text-muted-foreground">
                  {snapshot.audio.unavailable_reason}
                </p>
              )}
              <p className="text-xs text-muted-foreground">
                A currently unavailable TV output can be saved; Monarch will
                wait for it after enabling the display. Communications devices
                keep their Windows preference.
              </p>
              {snapshot.profiles.length === 0 ? (
                <div className="rounded-xl border border-dashed p-4 text-sm text-muted-foreground">
                  No profiles saved yet.
                </div>
              ) : (
                snapshot.profiles.map((profile, index) => {
                  const shortcutLabel = profileShortcutBase
                    ? indexedShortcutLabel(profileShortcutBase, index)
                    : (snapshot.settings.profile_shortcuts[profile.name] ??
                      null);

                  return (
                    <div
                      key={`${profile.name}:${profile.audio_output?.id ?? ""}`}
                      className="grid gap-3 rounded-xl border p-4 sm:grid-cols-[1fr_auto] sm:items-center"
                    >
                      <div className="space-y-1">
                        <h3 className="text-sm font-semibold leading-none text-foreground">
                          {profile.name}
                        </h3>
                        <p className="text-sm text-muted-foreground">
                          {
                            profile.layout.outputs.filter(
                              (output) => output.enabled,
                            ).length
                          }{" "}
                          active outputs
                        </p>
                        {shortcutLabel ? (
                          <p className="text-xs font-mono text-muted-foreground">
                            {shortcutsEnabled
                              ? "Shortcut"
                              : "Shortcut (disabled)"}
                            : {shortcutLabel}
                          </p>
                        ) : null}
                      </div>

                      <div className="flex flex-wrap gap-2 sm:justify-end">
                        <Button
                          type="button"
                          size="sm"
                          disabled={actionBusy || hasPendingConfirmation}
                          onClick={() => onApplyProfile(profile.name)}
                        >
                          Apply
                        </Button>
                        <Button
                          type="button"
                          size="sm"
                          variant="destructive"
                          disabled={actionBusy || hasPendingConfirmation}
                          onClick={() => onDeleteProfileRequest(profile.name)}
                        >
                          Delete
                        </Button>
                      </div>
                      <ProfileAudio
                        profile={profile}
                        audio={snapshot.audio}
                        disabled={actionBusy || hasPendingConfirmation}
                        onSave={onSaveProfileAudio}
                      />
                    </div>
                  );
                })
              )}
            </CardContent>
          </Card>
        </main>
      ) : null}
    </TabsContent>
  );
}
