import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { LayoutPreview } from "./layout-preview";
import { changeAttachment, editOutput } from "@/app/display-editor";
import type { AppSnapshot, Layout, OutputConfig } from "@/types";

const selectClass = "h-9 w-full rounded-md border bg-background px-2 text-sm disabled:opacity-50";
const rotations = [["landscape","Landscape"],["portrait","Portrait (90°)"],["landscape_flipped","Landscape (180°)"],["portrait_flipped","Portrait (270°)"]] as const;
export function DisplayEditor({ snapshot, initial, name, busy, onSave, onApply, onClose }: {
  snapshot: AppSnapshot; initial: Layout; name: string | null; busy: boolean;
  onSave: (layout: Layout) => void; onApply: (layout: Layout) => void; onClose: () => void;
}) {
  const [draft,setDraft] = useState(()=>structuredClone(initial));
  const [error,setError] = useState<string|null>(null);
  const preview = {...snapshot,layout:draft};
  const change = (key: string, patch: Partial<OutputConfig>) => setDraft(editOutput(draft,key,patch));
  return <div className="fixed inset-0 z-40 overflow-y-auto bg-background/95 p-4" role="dialog" aria-modal="true" aria-labelledby="display-editor-title">
    <section className="mx-auto max-w-5xl space-y-4 rounded-lg border bg-background p-5">
      <div className="flex items-center justify-between gap-3"><h2 id="display-editor-title" className="text-lg font-semibold">{name ? `Edit ${name}` : "Edit displays"}</h2><Button variant="outline" onClick={onClose} disabled={busy}>Cancel</Button></div>
      <p className="text-sm text-muted-foreground">{name ? "Save stores this profile. Apply previews these settings with a confirmation timer." : "Apply previews these settings with a confirmation timer."}</p>
      <LayoutPreview snapshot={preview}/>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      <fieldset disabled={busy} className="space-y-4">
      {draft.outputs.map((output,index)=> {
        const display = snapshot.displays.find((d)=>d.id_key === output.display_key);
        const cap = snapshot.capabilities.find((c)=>c.display_key === output.display_key);
        const portrait = output.rotation === "portrait" || output.rotation === "portrait_flipped";
        const width = portrait ? output.resolution.height : output.resolution.width;
        const height = portrait ? output.resolution.width : output.resolution.height;
        const modeKey = `${width}x${height}@${output.refresh_rate_mhz}`;
        const attachment = !output.enabled ? "detached" : output.clone_group ? draft.outputs.find((o)=>o !== output && o.enabled && o.clone_group === output.clone_group)?.display_key ?? "extend" : "extend";
        return <section className="space-y-3 border-t pt-4" key={output.display_key} aria-label={`Display ${index+1} settings`}>
          <h3 className="text-sm font-semibold">{index+1}. {display?.friendly_name ?? "Disconnected display"}</h3>
          {!display && <p className="text-sm text-muted-foreground">Disconnected. Saved settings are retained; reconnect this monitor before applying.</p>}
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            <label className="grid gap-1 text-sm">Display mode<select className={selectClass} value={attachment} onChange={(e)=>{try {setDraft(changeAttachment(draft,output.display_key,e.target.value));setError(null);} catch(e) {setError(String(e));}}}>
              <option value="extend">Extend</option><option value="detached">Detached</option>
              {draft.outputs.filter((o)=>o !== output && o.enabled).map((o)=><option key={o.display_key} value={o.display_key}>Duplicate of {draft.outputs.indexOf(o)+1}: {snapshot.displays.find((d)=>d.id_key===o.display_key)?.friendly_name ?? "saved display"}</option>)}
            </select></label>
            <label className="grid gap-1 text-sm">Resolution and refresh rate<select className={selectClass} value={modeKey} disabled={!cap?.modes.length} onChange={(e)=> {
              const mode = cap!.modes.find((m)=>`${m.resolution.width}x${m.resolution.height}@${m.refresh_rate_mhz}`===e.target.value)!;
              change(output.display_key,{resolution:portrait ? {width:mode.resolution.height,height:mode.resolution.width}:{...mode.resolution},refresh_rate_mhz:mode.refresh_rate_mhz});
            }}>
              {!cap?.modes.some((m)=>`${m.resolution.width}x${m.resolution.height}@${m.refresh_rate_mhz}`===modeKey) && <option value={modeKey}>{width} × {height} · {output.refresh_rate_mhz/1000} Hz (saved)</option>}
              {cap?.modes.map((m)=><option key={`${m.resolution.width}x${m.resolution.height}@${m.refresh_rate_mhz}`} value={`${m.resolution.width}x${m.resolution.height}@${m.refresh_rate_mhz}`}>{m.resolution.width} × {m.resolution.height} · {m.refresh_rate_mhz/1000} Hz</option>)}
            </select></label>
            <label className="grid gap-1 text-sm">Orientation<select className={selectClass} disabled={!cap?.modes.length} value={output.rotation ?? "landscape"} onChange={(e)=> {
              const rotation = e.target.value as OutputConfig["rotation"];
              const nextPortrait = rotation === "portrait" || rotation === "portrait_flipped";
              change(output.display_key,{rotation,resolution:portrait !== nextPortrait ? {width:output.resolution.height,height:output.resolution.width}:output.resolution});
            }}>{rotations.map(([value,label])=><option key={value} value={value}>{label}</option>)}</select></label>
            <label className="grid gap-1 text-sm">HDR<select className={selectClass} value={output.hdr_enabled == null ? "preserve":String(output.hdr_enabled)} onChange={(e)=>change(output.display_key,{hdr_enabled:e.target.value === "preserve" ? null:e.target.value === "true"})}>
              <option value="preserve">Preserve HDR</option><option value="true" disabled={!cap?.hdr_supported}>On</option><option value="false" disabled={!cap?.hdr_supported}>Off</option>
            </select></label>
            <label className="grid gap-1 text-sm">Scaling<select className={selectClass} value={output.scale_percent ?? "preserve"} onChange={(e)=>change(output.display_key,{scale_percent:e.target.value === "preserve" ? null:Number(e.target.value)})}>
              <option value="preserve">Preserve scaling</option>
              {output.scale_percent != null && !cap?.scale_percentages.includes(output.scale_percent) && <option value={output.scale_percent}>{output.scale_percent}% (saved)</option>}
              {cap?.scale_percentages.map((s)=><option key={s} value={s}>{s}%</option>)}
            </select></label>
            <div className="grid grid-cols-2 gap-2">{(["x","y"] as const).map((axis)=><label className="grid gap-1 text-sm" key={axis}>Position {axis.toUpperCase()}<Input type="number" value={output.position[axis]} min={-1000000} max={1000000} onChange={(e)=>change(output.display_key,{position:{...output.position,[axis]:Number(e.target.value)}})}/></label>)}</div>
          </div>
          {[cap?.modes_unavailable_reason,cap?.hdr_unavailable_reason,cap?.scaling_unavailable_reason].filter(Boolean).map((reason)=><p key={reason} className="text-xs text-muted-foreground">{reason}</p>)}
          <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={output.primary} disabled={!output.enabled || output.primary} onChange={()=>change(output.display_key,{primary:true})}/>Primary display</label>
        </section>;
      })}
      </fieldset>
      <div className="flex justify-end gap-2 border-t pt-4">
        {name && <Button variant="outline" disabled={busy} onClick={()=>onSave(draft)}>Save</Button>}
        <Button disabled={busy || Boolean(snapshot.pending_confirmation)} onClick={()=>onApply(draft)}>Apply</Button>
      </div>
    </section>
  </div>;
}
