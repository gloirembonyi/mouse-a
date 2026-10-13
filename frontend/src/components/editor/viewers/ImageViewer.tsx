import { useEffect, useRef, useState } from "react";
import clsx from "clsx";
import { Maximize, RotateCw, ZoomIn, ZoomOut } from "lucide-react";
import { IconButton } from "../../ui/primitives";
import { EmptyNotice } from "./common";
import type { StatusReporter } from "./util";

const MIN_ZOOM = 0.05;
const MAX_ZOOM = 16;
const STEP = 1.2;
const PAD = 48;

const clamp = (z: number) => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));

// Image viewer: fits by default; wheel / buttons / + - 0 keys zoom, drag pans
// once the image is bigger than the view, R rotates. The checkerboard sits
// only behind the image so transparency is obvious.
export function ImageViewer({ src, path, onStatus }: { src: string; path: string; onStatus: StatusReporter }) {
  const viewRef = useRef<HTMLDivElement>(null);
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [rotation, setRotation] = useState(0);
  const [view, setView] = useState({ w: 0, h: 0 });
  const [failed, setFailed] = useState(false);
  const drag = useRef<{ x: number; y: number; left: number; top: number } | null>(null);

  useEffect(() => {
    setNatural(null);
    setZoom("fit");
    setRotation(0);
    setFailed(false);
  }, [src]);

  useEffect(() => {
    const el = viewRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setView({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const sideways = rotation % 180 !== 0;
  const boxW = natural ? (sideways ? natural.h : natural.w) : 0;
  const boxH = natural ? (sideways ? natural.w : natural.h) : 0;
  const fitScale = natural && view.w ? Math.min((view.w - PAD) / boxW, (view.h - PAD) / boxH, 1) : 1;
  const scale = zoom === "fit" ? Math.max(fitScale, MIN_ZOOM) : zoom;

  const scaleRef = useRef(scale);
  scaleRef.current = scale;
  useEffect(() => {
    const el = viewRef.current;
    if (!el) return;
    // Native listener: React's wheel handler is passive and can't stop the page scrolling.
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      setZoom(clamp(scaleRef.current * (e.deltaY < 0 ? STEP : 1 / STEP)));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const onStatusRef = useRef(onStatus);
  onStatusRef.current = onStatus;
  useEffect(() => {
    onStatusRef.current(natural ? [`${natural.w} × ${natural.h}`, `${Math.round(scale * 100)}%`] : []);
  }, [natural, scale]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.key === "+" || e.key === "=") setZoom(clamp(scale * STEP));
    else if (e.key === "-") setZoom(clamp(scale / STEP));
    else if (e.key === "0") setZoom("fit");
    else if (e.key === "1") setZoom(1);
    else if (e.key.toLowerCase() === "r") setRotation((r) => (r + 90) % 360);
    else return;
    e.preventDefault();
  };

  const overflow = natural ? boxW * scale > view.w || boxH * scale > view.h : false;

  if (failed) {
    return <EmptyNotice title="Can't display this image" detail="The format or file looks damaged to the viewer." path={path} />;
  }

  return (
    <div className="relative flex min-h-0 flex-1 flex-col bg-bg-secondary">
      <div
        ref={viewRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onPointerDown={(e) => {
          const el = viewRef.current;
          if (!el || !overflow || e.button !== 0) return;
          el.setPointerCapture(e.pointerId);
          drag.current = { x: e.clientX, y: e.clientY, left: el.scrollLeft, top: el.scrollTop };
        }}
        onPointerMove={(e) => {
          const el = viewRef.current;
          if (!el || !drag.current) return;
          el.scrollLeft = drag.current.left - (e.clientX - drag.current.x);
          el.scrollTop = drag.current.top - (e.clientY - drag.current.y);
        }}
        onPointerUp={() => (drag.current = null)}
        onDoubleClick={() => setZoom((z) => (z === "fit" ? 1 : "fit"))}
        className={clsx("scrollbar-thin min-h-0 flex-1 overflow-auto outline-none", overflow && "cursor-grab active:cursor-grabbing")}
      >
        <div className="flex min-h-full min-w-full items-center justify-center" style={{ padding: PAD / 2, width: "max-content", height: "max-content" }}>
          <div
            className="relative rounded-sm bg-[repeating-conic-gradient(var(--mx-bg-tertiary)_0%_25%,var(--mx-bg-primary)_0%_50%)] bg-[length:16px_16px] shadow-pop"
            style={{ width: boxW * scale, height: boxH * scale }}
          >
            <img
              src={src}
              alt={path}
              draggable={false}
              onLoad={(e) => {
                const img = e.currentTarget;
                // Size-less SVGs report 0 - fall back to the browser default box.
                setNatural({ w: img.naturalWidth || 300, h: img.naturalHeight || 150 });
              }}
              onError={() => setFailed(true)}
              className={clsx("absolute left-1/2 top-1/2 max-w-none select-none", !natural && "invisible")}
              style={{
                width: (natural?.w ?? 0) * scale,
                height: (natural?.h ?? 0) * scale,
                transform: `translate(-50%, -50%) rotate(${rotation}deg)`,
                imageRendering: scale >= 3 ? "pixelated" : undefined,
              }}
            />
          </div>
        </div>
      </div>

      {natural && (
        <div className="absolute bottom-3 left-1/2 flex -translate-x-1/2 items-center gap-0.5 rounded-lg border border-border-default bg-bg-elevated p-0.5 shadow-pop">
          <IconButton title="Zoom out (-)" size="sm" onClick={() => setZoom(clamp(scale / STEP))}>
            <ZoomOut size={13} />
          </IconButton>
          <button
            onClick={() => setZoom(1)}
            title="Actual size (1)"
            className="h-6 min-w-[46px] rounded-md px-1 text-[11px] tabular-nums text-text-secondary transition hover:bg-bg-hover hover:text-text-primary"
          >
            {Math.round(scale * 100)}%
          </button>
          <IconButton title="Zoom in (+)" size="sm" onClick={() => setZoom(clamp(scale * STEP))}>
            <ZoomIn size={13} />
          </IconButton>
          <span className="mx-0.5 h-4 w-px bg-border-default" />
          <IconButton title="Fit to view (0)" size="sm" active={zoom === "fit"} onClick={() => setZoom("fit")}>
            <Maximize size={12} />
          </IconButton>
          <IconButton title="Rotate 90° (R)" size="sm" onClick={() => setRotation((r) => (r + 90) % 360)}>
            <RotateCw size={12} />
          </IconButton>
        </div>
      )}
    </div>
  );
}
