import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { TabsContent } from "@/components/ui/tabs";
import { LayoutPreview } from "@/app/components/layout-preview";
import { MonitorCard } from "@/app/components/monitor-card";
import { DisplayProperties } from "./display-properties";
import { editOutput, layoutError, rebaseLayout } from "@/app/display-editor";
import { indexedShortcutLabel } from "@/app/utils";
import type { AppSnapshot, DisplayInfo, Layout } from "@/types";

type MainTabProps = {
  loading: boolean;
  snapshot: AppSnapshot | null;
  activeDisplayCount: number;
  actionBusy: boolean;
  hasPendingConfirmation: boolean;
  shortcutsEnabled: boolean;
  displayShortcutBase: string | null;
  onApplyLayout: (layout: Layout) => Promise<boolean>;
  onRestoreLastLayout: () => void;
  onMakePrimaryRequest: (display: DisplayInfo) => void;
  onToggleRequest: (display: DisplayInfo) => void;
};

export function MainTab({
  loading,
  snapshot,
  activeDisplayCount,
  actionBusy,
  hasPendingConfirmation,
  shortcutsEnabled,
  displayShortcutBase,
  onApplyLayout,
  onRestoreLastLayout,
  onMakePrimaryRequest,
  onToggleRequest,
}: MainTabProps) {
  const [draft, setDraft] = useState<Layout | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const signature = JSON.stringify(snapshot?.layout);
  useEffect(() => {
    if (draft) {
      setDraft(null);
      if (!actionBusy)
        setNotice(
          "The active display layout changed. Review the current arrangement before editing again.",
        );
    }
    setEditing(null);
  }, [signature]);
  if (loading || !snapshot) {
    return <TabsContent value="main" className="mt-0" />;
  }

  const layout = draft ?? snapshot.layout;
  const error = draft ? layoutError(draft, snapshot.capabilities) : null;
  const locked = actionBusy || hasPendingConfirmation;
  function change(next: Layout) {
    setDraft(JSON.stringify(next) === signature ? null : next);
    setNotice(null);
  }

  return (
    <TabsContent value="main" className="mt-0">
      <main className="grid gap-4">
        <div className="w-full gap-4 lg:flex">
          <Card className="lg:w-2/3">
            <CardHeader className="gap-3 md:flex-row md:items-start md:justify-between">
              <CardTitle className="text-base">Layout Preview</CardTitle>
              <div className="flex flex-wrap items-center justify-end gap-2">
                <Badge variant="outline">
                  {layout.outputs.filter((o) => o.enabled).length} active
                </Badge>
                <Badge variant="secondary">
                  {snapshot.displays.length} detected
                </Badge>
              </div>
            </CardHeader>
            <CardContent>
              <LayoutPreview
                snapshot={{ ...snapshot, layout }}
                disabled={locked}
                onMove={(key, position) =>
                  change(editOutput(layout, key, { position }))
                }
              />
              <p className="mt-3 text-sm text-muted-foreground">
                Drag monitors to match your desk. Use arrow keys for small
                adjustments. Click a monitor in the list to change its
                properties.
              </p>
              {notice && (
                <p role="status" className="mt-2 text-sm text-muted-foreground">
                  {notice}
                </p>
              )}
              {error && (
                <p role="alert" className="mt-2 text-sm text-destructive">
                  {error}
                </p>
              )}
              <div className="mt-4 flex justify-end gap-2">
                <Button
                  variant="outline"
                  disabled={!draft || locked}
                  onClick={() => {
                    setDraft(null);
                    setNotice(null);
                  }}
                >
                  Discard changes
                </Button>
                <Button
                  disabled={!draft || locked || Boolean(error)}
                  onClick={() => {
                    setDraft(null);
                    setNotice(null);
                    void onApplyLayout(rebaseLayout(layout)).then((ok) => {
                      if (!ok) setDraft(layout);
                    });
                  }}
                >
                  Save layout
                </Button>
              </div>
            </CardContent>
          </Card>

          <Card className="mt-4 lg:mt-0 lg:w-1/3">
            <CardHeader className="gap-3 md:flex-row md:items-start md:justify-between">
              <CardTitle className="text-base">Monitors</CardTitle>
              <Button
                type="button"
                variant="outline"
                size="sm"
                disabled={actionBusy}
                onClick={onRestoreLastLayout}
              >
                Restore Last Layout
              </Button>
            </CardHeader>
            <CardContent className="grid max-h-[38rem] gap-3 overflow-auto pr-1">
              {snapshot.displays.map((display, index) => {
                const shortcutLabel = displayShortcutBase
                  ? indexedShortcutLabel(displayShortcutBase, index)
                  : (snapshot.settings.display_toggle_shortcuts[
                      display.id_key
                    ] ?? null);

                return (
                  <MonitorCard
                    key={display.id_key}
                    display={display}
                    monitorNumber={index + 1}
                    shortcutLabel={shortcutLabel}
                    shortcutsEnabled={shortcutsEnabled}
                    busy={actionBusy || Boolean(draft)}
                    hasPendingConfirmation={hasPendingConfirmation}
                    activeDisplayCount={activeDisplayCount}
                    onMakePrimaryRequest={onMakePrimaryRequest}
                    onToggleRequest={onToggleRequest}
                    onEdit={() => setEditing(display.id_key)}
                    editDisabled={locked}
                  />
                );
              })}
            </CardContent>
          </Card>
        </div>

        {editing && (
          <DisplayProperties
            key={editing}
            displayKey={editing}
            initial={layout}
            snapshot={snapshot}
            busy={locked}
            onClose={() => setEditing(null)}
            onSave={(next) => {
              change(next);
              setEditing(null);
            }}
          />
        )}

        <Card className="border-dashed">
          <CardContent className="space-y-2 p-4">
            <p className="text-sm font-medium text-foreground">
              Troubleshooting
            </p>
            <p className="text-sm text-muted-foreground">
              If something goes wrong or monitors are missing or not showing up
              as expected, press{" "}
              <span className="font-medium text-foreground">Win + P</span> and
              choose
              <span className="font-medium text-foreground"> Extend</span> or
              <span className="font-medium text-foreground">
                {" "}
                PC screen only
              </span>{" "}
              to reset the display mode.
            </p>
          </CardContent>
        </Card>
      </main>
    </TabsContent>
  );
}
