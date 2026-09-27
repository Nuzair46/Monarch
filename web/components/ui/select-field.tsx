import { useId } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./select";

export type SelectChoice = { value: string; label: string; disabled?: boolean };

export function SelectField({
  label,
  value,
  choices,
  onValueChange,
  disabled,
  className,
}: {
  label: string;
  value: string;
  choices: SelectChoice[];
  onValueChange: (value: string) => void;
  disabled?: boolean;
  className?: string;
}) {
  const id = useId();
  return (
    <div className={`grid min-w-0 gap-1.5 ${className ?? ""}`}>
      <label htmlFor={id} className="field-label">
        {label}
      </label>
      <Select value={value} onValueChange={onValueChange} disabled={disabled}>
        <SelectTrigger id={id} aria-label={label}>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {choices.map((choice) => (
            <SelectItem
              key={choice.value}
              value={choice.value}
              disabled={choice.disabled}
            >
              {choice.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
