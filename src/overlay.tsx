import "./shell";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface Payload {
  bands: Rect[];
  zones: Rect[];
}

function Overlay() {
  const [bands, setBands] = useState<Rect[]>([]);
  const [zones, setZones] = useState<Rect[]>([]);

  useEffect(() => {
    const apply = (payload: Payload) => {
      // The payload is in physical pixels; CSS pixels are physical / dpr.
      const ratio = window.devicePixelRatio || 1;
      const scale = (rect: Rect): Rect => ({
        x: rect.x / ratio,
        y: rect.y / ratio,
        w: rect.w / ratio,
        h: rect.h / ratio,
      });
      setBands((payload.bands ?? []).map(scale));
      setZones((payload.zones ?? []).map(scale));
    };

    // The window is created before this page finishes loading, so the event that
    // triggered it has already been emitted: ask for the current zones as well.
    void invoke<Payload>("overlay_zones").then(apply);

    const unlisten = listen<Payload>("overlay-zones", (event) => apply(event.payload));
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  return (
    <>
      {bands.map((band, index) => (
        <div
          key={`band-${index}`}
          className="band"
          style={{ left: band.x, top: band.y, width: band.w, height: band.h }}
        />
      ))}
      {zones.map((zone, index) => (
        <div
          key={`zone-${index}`}
          className="zone"
          style={{ left: zone.x, top: zone.y, width: zone.w, height: zone.h }}
        />
      ))}
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Overlay />);
