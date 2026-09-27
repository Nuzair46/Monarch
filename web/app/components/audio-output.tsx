import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
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
  const id = useId();
  const devices = [...audio.devices].sort(
    (a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id),
  );
  const available = devices.filter((device) => device.available);
  const unavailable = devices.filter((device) => !device.available);
  const missing = value && !audio.devices.some((device) => device.id === value);
  return (
    <div className="grid min-w-0 gap-1.5">
      <label htmlFor={id} className="field-label">
        Audio output
      </label>
      <Select
        value={value ?? "__unchanged__"}
        disabled={disabled}
        onValueChange={(next) =>
          onChange(next === "__unchanged__" ? null : next)
        }
      >
        <SelectTrigger id={id} aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__unchanged__">Leave unchanged</SelectItem>
          {available.length > 0 && (
            <SelectGroup>
              <SelectSeparator />
              <SelectLabel>Available</SelectLabel>
              {available.map((device) => (
                <SelectItem key={device.id} value={device.id}>
                  {device.name}
                </SelectItem>
              ))}
            </SelectGroup>
          )}
          {(unavailable.length > 0 || missing) && (
            <SelectGroup>
              <SelectSeparator />
              <SelectLabel>Unavailable</SelectLabel>
              {missing && (
                <SelectItem value={value} disabled>
                  {saved?.name ?? "Saved audio output"} (unavailable)
                </SelectItem>
              )}
              {unavailable.map((device) => (
                <SelectItem key={device.id} value={device.id}>
                  {device.name} (unavailable)
                </SelectItem>
              ))}
            </SelectGroup>
          )}
        </SelectContent>
      </Select>
    </div>
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
    <div className="flex min-w-0 flex-wrap items-end gap-2">
      <div className="min-w-0 basis-56 grow sm:max-w-sm">
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
            disabled={disabled}
            onClick={() => onSave(profile.name, selected)}
          >
            Save audio
          </Button>
          <Button
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
