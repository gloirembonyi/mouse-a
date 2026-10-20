import { useEffect, useState } from "react";
import { Switch } from "../ui/primitives";
import {
  onStealthChanged,
  onWindowPrefs,
  stealthGet,
  stealthSet,
  windowPrefsGet,
  windowPrefsSet,
  type WindowPrefs,
} from "../../lib/tauri";

/** How Kudu Desktop sits on the desktop: taskbar button, launch behavior and
 *  screen-share visibility. The tray menu flips the same switches. */
export function DesktopSection() {
  const [prefs, setPrefs] = useState<WindowPrefs>({ hide_from_taskbar: false, start_in_background: false, hide_from_screen_share: true, share_usage_stats: true });
  const [stealth, setStealth] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    windowPrefsGet().then(setPrefs).catch(() => {});
    stealthGet().then(setStealth).catch(() => {});
    const unPrefs = onWindowPrefs(setPrefs);
    const unStealth = onStealthChanged(setStealth);
    return () => {
      unPrefs.then((fn) => fn());
      unStealth.then((fn) => fn());
    };
  }, []);

  const update = (patch: Partial<WindowPrefs>) => {
    const next = { ...prefs, ...patch };
    setPrefs(next);
    setError(null);
    windowPrefsSet(next).catch((err) => {
      setError(err instanceof Error ? err.message : String(err));
      setPrefs(prefs);
    });
  };

  return (
    <section className="flex flex-col gap-3">
      <div>
        <h3 className="text-[13.5px] font-semibold">Desktop</h3>
        <p className="mt-0.5 text-[12px] text-text-muted">
          Kudu Desktop keeps running in the tray (the ^ arrow next to the clock) when its window is closed.
        </p>
      </div>
      <Row
        title="Show in taskbar"
        hint="Off: no taskbar button - open Kudu Desktop from the tray icon or a shortcut."
        on={!prefs.hide_from_taskbar}
        onChange={(v) => update({ hide_from_taskbar: !v })}
      />
      <Row
        title="Start in the background"
        hint="Launch straight to the tray instead of opening this window."
        on={prefs.start_in_background}
        onChange={(v) => update({ start_in_background: v })}
      />
      <Row
        title="Hide from screen share"
        hint="No Kudu Desktop window (this one, the Kudu Lens overlay, the Pilot bar, the working banner) appears in shared screens or recordings. Remembered across launches."
        on={stealth}
        onChange={(v) => void stealthSet(v).then(setStealth)}
      />
      <Row
        title="Share anonymous usage statistics"
        hint="Which features get used (e.g. an agent task finished), so we know what to improve. Never your screen, what you type or say, answers or file names."
        on={prefs.share_usage_stats !== false}
        onChange={(v) => update({ share_usage_stats: v })}
      />
      {error && <p className="text-[12px] text-danger">{error}</p>}
    </section>
  );
}

function Row({ title, hint, on, onChange }: { title: string; hint: string; on: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-4 rounded-xl border border-border-soft bg-bg-card px-3.5 py-2.5">
      <span className="flex flex-col">
        <span className="text-[13px] text-text-primary">{title}</span>
        <span className="text-[11.5px] text-text-muted">{hint}</span>
      </span>
      <Switch on={on} onChange={onChange} label={title} />
    </label>
  );
}
