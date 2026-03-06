import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { TabsContent } from "@/components/ui/tabs";
import { LayoutPreview } from "@/app/components/layout-preview";
import { MonitorCard } from "@/app/components/monitor-card";
import { indexedShortcutLabel } from "@/app/utils";
import type { AppSnapshot, DisplayInfo } from "@/types";

type MainTabProps = {
  loading: boolean;
  snapshot: AppSnapshot | null;
  activeDisplayCount: number;
  actionBusy: boolean;
  hasPendingConfirmation: boolean;
  shortcutsEnabled: boolean;
  displayShortcutBase: string | null;
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
  onRestoreLastLayout,
  onMakePrimaryRequest,
  onToggleRequest,
}: MainTabProps) {
  if (loading || !snapshot) {
    return <TabsContent value="main" className="mt-0" />;
  }

  return (
    <TabsContent value="main" className="mt-0">
      <main className="grid gap-4">
        <div className="w-full gap-4 lg:flex">
          <Card className="lg:w-2/3">
            <CardHeader className="gap-3 md:flex-row md:items-start md:justify-between">
              <CardTitle className="text-base">Layout Preview</CardTitle>
              <div className="flex flex-wrap items-center justify-end gap-2">
                <Badge variant="outline">{activeDisplayCount} active</Badge>
                <Badge variant="secondary">{snapshot.displays.length} detected</Badge>
              </div>
            </CardHeader>
            <CardContent>
              <LayoutPreview snapshot={snapshot} />
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
                const shortcutLabel = indexedShortcutLabel(displayShortcutBase, index);

                return (
                  <MonitorCard
                    key={display.id_key}
                    display={display}
                    monitorNumber={index + 1}
                    shortcutLabel={shortcutLabel}
                    shortcutsEnabled={shortcutsEnabled}
                    busy={actionBusy}
                    hasPendingConfirmation={hasPendingConfirmation}
                    activeDisplayCount={activeDisplayCount}
                    onMakePrimaryRequest={onMakePrimaryRequest}
                    onToggleRequest={onToggleRequest}
                  />
                );
              })}
            </CardContent>
          </Card>
        </div>

        <Card className="border-dashed">
          <CardContent className="space-y-2 p-4">
            <p className="text-sm font-medium text-foreground">Troubleshooting</p>
            <p className="text-sm text-muted-foreground">
              If something goes wrong or monitors are missing or not showing up as expected, press{" "}
              <span className="font-medium text-foreground">Win + P</span> and choose
              <span className="font-medium text-foreground"> Extend</span> or
              <span className="font-medium text-foreground"> PC screen only</span> to reset the
              display mode.
            </p>
          </CardContent>
        </Card>
      </main>
    </TabsContent>
  );
}
