import { Layers, Plus, Play, Trash2 } from "lucide-react";
import { AudioOutputSelect, ProfileAudio } from "./audio-output";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
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
  const locked = actionBusy || hasPendingConfirmation;
  return (
    <TabsContent value="profiles" className="mt-0">
      {!loading && snapshot ? (
        <main className="grid items-start gap-4 min-[900px]:grid-cols-[minmax(0,1fr)_340px]">
          <Card className="min-[900px]:col-start-2 min-[900px]:row-start-1">
            <CardHeader className="min-h-12 flex-row items-center py-2">
              <CardTitle className="flex items-center gap-2">
                <Plus
                  className="size-4 text-muted-foreground"
                  aria-hidden="true"
                />
                Save current setup
              </CardTitle>
            </CardHeader>
            <CardContent>
              <form
                className="grid gap-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (!locked && newProfileName.trim()) onSaveCurrentLayout();
                }}
              >
                <p className="text-xs leading-relaxed text-muted-foreground">
                  Capture your current display layout and choose an optional
                  audio output.
                </p>
                <label className="grid gap-1.5">
                  <span className="field-label">Profile name</span>
                  <Input
                    type="text"
                    placeholder="e.g. Work, Gaming, TV"
                    value={newProfileName}
                    disabled={locked}
                    onChange={(event) =>
                      onNewProfileNameChange(event.target.value)
                    }
                  />
                </label>
                <AudioOutputSelect
                  label="Audio output for new profile"
                  audio={snapshot.audio}
                  value={newProfileAudio}
                  onChange={onNewProfileAudioChange}
                  disabled={locked}
                />
                <Button
                  type="submit"
                  disabled={locked || !newProfileName.trim()}
                >
                  Save Current Layout
                </Button>
                <p className="text-xs leading-relaxed text-muted-foreground">
                  Saving a profile leaves your active displays and audio
                  unchanged.
                </p>
              </form>
            </CardContent>
          </Card>
          <Card className="min-[900px]:col-start-1 min-[900px]:row-start-1">
            <CardHeader className="min-h-12 flex-row items-center justify-between py-2">
              <CardTitle className="flex items-center gap-2">
                <Layers
                  className="size-4 text-muted-foreground"
                  aria-hidden="true"
                />
                Saved profiles
              </CardTitle>
              <span className="text-xs text-muted-foreground">
                {snapshot.profiles.length} saved
              </span>
            </CardHeader>
            <CardContent className="divide-y p-0">
              {snapshot.profiles.length === 0 ? (
                <div className="space-y-2 px-4 py-10 text-center">
                  <p className="text-sm font-medium">No profiles yet</p>
                  <p className="text-xs text-muted-foreground">
                    Arrange your displays, then save your first setup.
                  </p>
                </div>
              ) : (
                snapshot.profiles.map((profile, index) => {
                  const activeCount = profile.layout.outputs.filter(
                    (output) => output.enabled,
                  ).length;
                  const shortcutLabel = profileShortcutBase
                    ? indexedShortcutLabel(profileShortcutBase, index)
                    : (snapshot.settings.profile_shortcuts[profile.name] ??
                      null);
                  return (
                    <article
                      key={`${profile.name}:${profile.audio_output?.id ?? ""}`}
                      aria-label={`Profile ${profile.name}`}
                      className="grid min-w-0 gap-4 p-4"
                    >
                      <div className="flex flex-wrap items-start justify-between gap-3">
                        <div className="min-w-0 flex-1 space-y-1.5">
                          <h3 className="break-words text-sm font-medium">
                            {profile.name}
                          </h3>
                          <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
                            <span>
                              {activeCount} active{" "}
                              {activeCount === 1 ? "display" : "displays"}
                            </span>
                            {shortcutLabel && (
                              <kbd
                                className="shortcut"
                                title={
                                  shortcutsEnabled
                                    ? "Apply this profile"
                                    : "Global shortcuts are disabled"
                                }
                              >
                                {shortcutLabel}
                                {!shortcutsEnabled && " · off"}
                              </kbd>
                            )}
                          </div>
                        </div>
                        <div className="flex gap-2">
                          <Button
                            type="button"
                            size="sm"
                            variant="secondary"
                            disabled={locked}
                            onClick={() => onApplyProfile(profile.name)}
                          >
                            <Play aria-hidden="true" />
                            Apply
                          </Button>
                          <Button
                            type="button"
                            size="icon"
                            variant="ghost"
                            className="h-8 w-8"
                            disabled={locked}
                            aria-label={`Delete profile ${profile.name}`}
                            title="Delete profile"
                            onClick={() => onDeleteProfileRequest(profile.name)}
                          >
                            <Trash2 aria-hidden="true" />
                          </Button>
                        </div>
                      </div>
                      <ProfileAudio
                        profile={profile}
                        audio={snapshot.audio}
                        disabled={locked}
                        onSave={onSaveProfileAudio}
                      />
                    </article>
                  );
                })
              )}
            </CardContent>
          </Card>
          <p className="text-xs leading-relaxed text-muted-foreground min-[900px]:col-span-2">
            {snapshot.audio.unavailable_reason ??
              "Unavailable TV audio outputs can be saved; Monarch waits for them when the display activates. Communications devices keep their Windows preference."}
          </p>
        </main>
      ) : null}
    </TabsContent>
  );
}
