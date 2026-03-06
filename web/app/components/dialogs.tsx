import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { formatMs } from "@/app/utils";
import type { PendingDisplayToggle } from "@/app/ui";
import type { PendingConfirmation } from "@/types";

const noop = () => {};

type PendingConfirmationDialogProps = {
  pendingConfirmation: PendingConfirmation | null;
  busy: boolean;
  onConfirm: () => void;
  onRevert: () => void;
};

export function PendingConfirmationDialog({
  pendingConfirmation,
  busy,
  onConfirm,
  onRevert,
}: PendingConfirmationDialogProps) {
  return (
    <AlertDialog open={Boolean(pendingConfirmation)} onOpenChange={noop}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Confirm layout change</AlertDialogTitle>
          <AlertDialogDescription>
            {pendingConfirmation ? (
              <>
                Automatic rollback in{" "}
                <span className="font-medium text-foreground">
                  {formatMs(pendingConfirmation.remaining_ms)}
                </span>
                . Confirm to keep this layout or revert now.
              </>
            ) : null}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy} onClick={onRevert}>
            Revert
          </AlertDialogCancel>
          <AlertDialogAction disabled={busy} onClick={onConfirm}>
            Confirm
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

type DisplayToggleDialogProps = {
  pendingDisplayToggle: PendingDisplayToggle | null;
  busy: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
};

export function DisplayToggleDialog({
  pendingDisplayToggle,
  busy,
  onOpenChange,
  onConfirm,
}: DisplayToggleDialogProps) {
  const isActive = Boolean(pendingDisplayToggle?.currentlyActive);
  const actionLabel = isActive ? "Detach Display" : "Attach Display";
  const title = isActive ? "Detach display?" : "Attach display?";
  const description = isActive
    ? "will be removed from the active layout."
    : "will be added back to the active layout.";

  return (
    <AlertDialog open={Boolean(pendingDisplayToggle)} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>
            {pendingDisplayToggle ? (
              <>
                <span className="font-medium text-foreground">
                  {pendingDisplayToggle.friendlyName}
                </span>{" "}
                {description}
              </>
            ) : null}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant={isActive ? "destructive" : "default"}
            disabled={busy}
            onClick={onConfirm}
          >
            {actionLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

type DeleteProfileDialogProps = {
  pendingProfileDelete: string | null;
  busy: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
};

export function DeleteProfileDialog({
  pendingProfileDelete,
  busy,
  onOpenChange,
  onConfirm,
}: DeleteProfileDialogProps) {
  return (
    <AlertDialog open={Boolean(pendingProfileDelete)} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete profile?</AlertDialogTitle>
          <AlertDialogDescription>
            {pendingProfileDelete ? (
              <>
                This will permanently delete the profile{" "}
                <span className="font-medium text-foreground">{pendingProfileDelete}</span>.
                This action cannot be undone.
              </>
            ) : null}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={busy}
            onClick={onConfirm}
          >
            Delete Profile
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
