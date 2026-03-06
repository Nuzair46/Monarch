import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatHz } from "@/app/utils";
import type { DisplayInfo } from "@/types";

type MonitorCardProps = {
  display: DisplayInfo;
  monitorNumber: number;
  shortcutLabel: string | null;
  shortcutsEnabled: boolean;
  busy: boolean;
  hasPendingConfirmation: boolean;
  activeDisplayCount: number;
  onMakePrimaryRequest: (display: DisplayInfo) => void;
  onToggleRequest: (display: DisplayInfo) => void;
};

export function MonitorCard({
  display,
  monitorNumber,
  shortcutLabel,
  shortcutsEnabled,
  busy,
  hasPendingConfirmation,
  activeDisplayCount,
  onMakePrimaryRequest,
  onToggleRequest,
}: MonitorCardProps) {
  const isActive = display.is_active;
  const isPrimary = display.is_primary;
  const actionLabel = display.is_active ? "Detach Display" : "Attach Display";
  const toggleDisabled =
    busy || hasPendingConfirmation || (isActive && activeDisplayCount <= 1);
  const resolutionLabel = `${display.resolution.width} x ${display.resolution.height}`;

  return (
    <article className="rounded-xl border p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
            Monitor {monitorNumber}
          </p>
          <h3 className="truncate text-sm font-semibold leading-none text-foreground">
            {display.friendly_name}
          </h3>
          <p className="text-sm text-muted-foreground">
            {resolutionLabel} · {formatHz(display.refresh_rate_mhz)}
          </p>
          {shortcutLabel ? (
            <p className="text-xs font-mono text-muted-foreground">
              {shortcutsEnabled ? "Shortcut" : "Shortcut (disabled)"}: {shortcutLabel}
            </p>
          ) : null}
        </div>

        <div className="flex flex-wrap items-center justify-end gap-2">
          <Badge variant={isActive ? "default" : "secondary"}>
            {isActive ? "Active" : "Detached"}
          </Badge>
          {isPrimary ? (
            <Badge variant="outline">Primary</Badge>
          ) : null}
        </div>
      </div>

      <div className="mt-4 flex flex-wrap items-center justify-end gap-2 border-t pt-3">
        {isActive && !isPrimary ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={busy || hasPendingConfirmation}
            onClick={() => onMakePrimaryRequest(display)}
          >
            Make Primary
          </Button>
        ) : null}
        <Button
          type="button"
          size="sm"
          variant={isActive ? "destructive" : "default"}
          disabled={toggleDisabled}
          onClick={() => onToggleRequest(display)}
        >
          {actionLabel}
        </Button>
      </div>
    </article>
  );
}
