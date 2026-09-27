import { useState } from "react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import type { AudioOutput, AudioSnapshot, Profile } from "@/types";

export function AudioOutputSelect({
  audio,
  value,
  saved,
  onChange,
  disabled,
  activeOnly = false,
  label = "Audio output",
}: {
  audio: AudioSnapshot;
  value: string | null;
  saved?: AudioOutput | null;
  onChange: (id: string | null) => void;
  disabled: boolean;
  activeOnly?: boolean;
  label?: string;
}) {
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
        <option value="">
          {activeOnly ? "Select audio output" : "Leave unchanged"}
        </option>
        {missing && (
          <option value={value} disabled>
            {saved?.name ?? "Saved audio output"} (unavailable)
          </option>
        )}
        {audio.devices.map((device) => (
          <option
            key={device.id}
            value={device.id}
            disabled={activeOnly && !device.available}
          >
            {device.name}
            {device.available ? "" : " (currently unavailable)"}
          </option>
        ))}
      </select>
    </label>
  );
}

export function AudioOutputCard({
  audio,
  disabled,
  onApply,
}: {
  audio: AudioSnapshot;
  disabled: boolean;
  onApply: (id: string) => void;
}) {
  const [selected, setSelected] = useState(audio.defaults.console);
  const available = audio.devices.some(
    (device) => device.id === selected && device.available,
  );
  const changed =
    selected !== audio.defaults.console ||
    selected !== audio.defaults.multimedia;
  return (
    <Card>
      <CardHeader>
        <CardTitle className="text-base">Audio output</CardTitle>
        <CardDescription>
          Choose the Windows playback output for system sounds and media.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-2">
        <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
          <div className="w-full sm:max-w-md">
            <AudioOutputSelect
              audio={audio}
              value={selected}
              onChange={setSelected}
              activeOnly
              disabled={disabled || Boolean(audio.unavailable_reason)}
              label="Playback device"
            />
          </div>
          <Button
            disabled={
              disabled ||
              !available ||
              !changed ||
              Boolean(audio.unavailable_reason)
            }
            onClick={() => selected && onApply(selected)}
          >
            Switch output
          </Button>
        </div>
        {audio.unavailable_reason ? (
          <p className="text-sm text-muted-foreground">
            {audio.unavailable_reason}
          </p>
        ) : !audio.devices.some((device) => device.available) ? (
          <p className="text-sm text-muted-foreground">
            No audio outputs are available. Connect or enable a device in
            Windows Sound settings.
          </p>
        ) : null}
        <p className="text-xs text-muted-foreground">
          You can also choose an output for each profile. Apps with their own
          device selection may keep using that device.
        </p>
      </CardContent>
    </Card>
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
