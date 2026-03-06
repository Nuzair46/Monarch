const MAX_DIGIT_SHORTCUTS = 10;

export function formatHz(mhz: number): string {
  return `${(mhz / 1000).toFixed(0)} Hz`;
}

export function formatMs(ms: number): string {
  return `${Math.max(0, Math.ceil(ms / 1000))}s`;
}

export function capitalizeToastError(message: string): string {
  const trimmed = message.trim();
  if (!trimmed) {
    return "Error";
  }

  return trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
}

export function shortcutSlotKey(index: number): string | null {
  if (index >= 0 && index < MAX_DIGIT_SHORTCUTS - 1) {
    return String(index + 1);
  }
  if (index === MAX_DIGIT_SHORTCUTS - 1) {
    return "0";
  }
  return null;
}

export function indexedShortcutLabel(
  base: string | null | undefined,
  index: number,
): string | null {
  const trimmedBase = (base ?? "").trim();
  if (!trimmedBase) {
    return null;
  }

  const slot = shortcutSlotKey(index);
  if (!slot) {
    return null;
  }

  return `${trimmedBase}+${slot}`;
}
