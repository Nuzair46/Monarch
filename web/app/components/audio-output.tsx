import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Volume2 } from "lucide-react";
import type { AudioOutput, AudioSnapshot, Profile } from "@/types";

export function AudioOutputSelect({
  audio,
  value,
  saved,
  onChange,
  disabled,
  label = "Audio output",
}: {
  audio: AudioSnapshot;
  value: string | null;
  saved?: AudioOutput | null;
  onChange: (id: string | null) => void;
  disabled: boolean;
  label?: string;
}) {
  const devices = [...audio.devices].sort(
    (a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id),
  );
  const available = devices.filter((device) => device.available);
  const unavailable = devices.filter((device) => !device.available);
  const missing = value && !audio.devices.some((device) => device.id === value);
  return (
    <label className="grid min-w-0 gap-1 text-sm">
      {label}
      <select
        aria-label={label}
        className="h-10 w-full min-w-0 rounded-md border bg-background px-2 text-sm disabled:opacity-50"
        value={value ?? ""}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value || null)}
      >
        <option value="">Leave unchanged</option>
        {available.length > 0 && (
          <optgroup label="Available">
            {available.map((device) => (
              <option key={device.id} value={device.id}>
                {device.name}
              </option>
            ))}
          </optgroup>
        )}
        {(unavailable.length > 0 || missing) && (
          <optgroup label="Unavailable">
            {missing && (
              <option value={value} disabled>
                {saved?.name ?? "Saved audio output"} (unavailable)
              </option>
            )}
            {unavailable.map((device) => (
              <option key={device.id} value={device.id}>
                {device.name} (unavailable)
              </option>
            ))}
          </optgroup>
        )}
      </select>
    </label>
  );
}

export function CurrentAudioOutput({ audio }: { audio: AudioSnapshot }) {
  const current = audio.devices.find(
    (device) => device.id === audio.defaults.console,
  );
  const name = audio.unavailable_reason
    ? "Unavailable"
    : (current?.name ??
      (audio.defaults.console ? "Device unavailable" : "No output device"));
  return (
    <p
      role="status"
      className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground"
      title={audio.unavailable_reason ?? name}
    >
      <Volume2 className="size-3.5 shrink-0" aria-hidden="true" />
      <span className="truncate">Audio: {name}</span>
    </p>
  );
}

export function ProfileAudio({
  profile,
  audio,
  disabled,
  onSave,
}: {
  profile: Profile;
  audio: AudioSnapshot;
  disabled: boolean;
  onSave: (name: string, id: string | null) => void;
}) {
  const saved = profile.audio_output?.id ?? null;
  const [selected, setSelected] = useState(saved);
  return (
    <div className="flex min-w-0 flex-col gap-2 sm:col-span-2 sm:flex-row sm:items-end">
      <div className="w-full sm:max-w-md">
        <AudioOutputSelect
          label={`Audio output for ${profile.name}`}
          audio={audio}
          value={selected}
          saved={profile.audio_output}
          onChange={setSelected}
          disabled={disabled}
        />
      </div>
      {selected !== saved && (
        <>
          <Button
            size="sm"
            disabled={disabled}
            onClick={() => onSave(profile.name, selected)}
          >
            Save audio
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={disabled}
            onClick={() => setSelected(saved)}
          >
            Cancel
          </Button>
        </>
      )}
    </div>
  );
}
