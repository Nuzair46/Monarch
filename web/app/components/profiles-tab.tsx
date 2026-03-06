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
                  Save named layouts and apply them later in one action.
                </CardDescription>
              </div>

              <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
                <Input
                  type="text"
                  placeholder="Profile name"
                  value={newProfileName}
                  onChange={(event) => onNewProfileNameChange(event.target.value)}
                  className="sm:max-w-sm"
                />
                <Button
                  type="button"
                  disabled={actionBusy || !newProfileName.trim() || hasPendingConfirmation}
                  onClick={onSaveCurrentLayout}
                >
                  Save Current Layout
                </Button>
              </div>
            </CardHeader>

            <CardContent className="grid gap-3">
              {snapshot.profiles.length === 0 ? (
                <div className="rounded-xl border border-dashed p-4 text-sm text-muted-foreground">
                  No profiles saved yet.
                </div>
              ) : (
                snapshot.profiles.map((profile, index) => {
                  const shortcutLabel = indexedShortcutLabel(profileShortcutBase, index);

                  return (
                    <div
                      key={profile.name}
                      className="grid gap-3 rounded-xl border p-4 sm:grid-cols-[1fr_auto] sm:items-center"
                    >
                      <div className="space-y-1">
                        <h3 className="text-sm font-semibold leading-none text-foreground">
                          {profile.name}
                        </h3>
                        <p className="text-sm text-muted-foreground">
                          {profile.layout.outputs.filter((output) => output.enabled).length}{" "}
                          active outputs
                        </p>
                        {shortcutLabel ? (
                          <p className="text-xs font-mono text-muted-foreground">
                            {shortcutsEnabled ? "Shortcut" : "Shortcut (disabled)"}: {shortcutLabel}
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
