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
