import { useEffect, useRef, useState } from "react";
import { Music } from "lucide-react";
import { assetUrl } from "../../../lib/tauri";
import { EmptyNotice } from "./common";
import type { StatusReporter } from "./util";

function formatDuration(sec: number) {
  if (!Number.isFinite(sec)) return "";
  const s = Math.round(sec);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h ? `${h}:${pad(m)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

// Audio / video straight from disk via the asset protocol (the webview
// streams and seeks it - nothing goes through IPC). WebView2 plays the common
// web formats; anything else (most .mkv, some .mov codecs) gets a way out.
export function MediaViewer({ kind, path, onStatus }: { kind: "audio" | "video"; path: string; onStatus: StatusReporter }) {
  const [failed, setFailed] = useState(false);
  const src = assetUrl(path);
  const name = path.split(/[\\/]/).pop() ?? path;
  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;

  useEffect(() => setFailed(false), [path]);

  if (failed) {
    return (
      <EmptyNotice
        title={`Can't play this ${kind} here`}
        detail="The built-in player doesn't support this format or codec. Your default media player will."
        path={path}
      />
    );
  }

  if (kind === "video") {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center bg-bg-secondary p-4">
        <video
          key={src}
          src={src}
          controls
          preload="metadata"
          onError={() => setFailed(true)}
          onLoadedMetadata={(e) => {
            const v = e.currentTarget;
            onStatusRef.current([`${v.videoWidth} × ${v.videoHeight}`, formatDuration(v.duration)].filter(Boolean));
          }}
          className="max-h-full max-w-full rounded-lg bg-bg-tertiary shadow-pop outline-none"
        />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center bg-bg-secondary p-6">
      <div className="flex w-full max-w-[440px] flex-col items-center gap-4 rounded-2xl border border-border-default bg-bg-elevated px-6 py-7 shadow-pop">
        <div className="flex h-16 w-16 items-center justify-center rounded-2xl bg-accent-soft text-accent">
          <Music size={28} />
        </div>
        <p className="max-w-full truncate text-[13px] font-medium text-text-primary" title={name}>
          {name}
        </p>
        <audio
          key={src}
          src={src}
          controls
          preload="metadata"
          onError={() => setFailed(true)}
          onLoadedMetadata={(e) => onStatusRef.current([formatDuration(e.currentTarget.duration)].filter(Boolean))}
          className="w-full"
        />
      </div>
    </div>
  );
}
