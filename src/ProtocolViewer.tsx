import { useLayoutEffect, useRef, useState } from "react";
import { ServerToClientMessage } from "./protocol";

export type ProtocolLogEntry = { id: number; receivedAt: Date } & (
  | { ok: true; message: ServerToClientMessage }
  | { ok: false; raw: unknown; error: string }
);

function entryLabel(entry: ProtocolLogEntry): string {
  if (!entry.ok) {
    return "Invalid message";
  }
  const { message } = entry;
  return message.type === "UdpTunnel" ? `UdpTunnel/${message.data.type}` : message.type;
}

function isAudio(entry: ProtocolLogEntry): boolean {
  return entry.ok && entry.message.type === "UdpTunnel" && entry.message.data.type === "Audio";
}

export function ProtocolViewer({ entries, onClear }: { entries: ProtocolLogEntry[]; onClear: () => void }) {
  // Voice packets arrive ~50/s per speaker and drown out everything else.
  const [showAudio, setShowAudio] = useState(false);
  const visible = showAudio ? entries : entries.filter((entry) => !isAudio(entry));

  // Follow new entries, unless the user has scrolled up to read older ones.
  const listRef = useRef<HTMLOListElement>(null);
  const followingRef = useRef(true);
  const lastVisibleId = visible[visible.length - 1]?.id;
  useLayoutEffect(() => {
    const list = listRef.current;
    if (list && followingRef.current) {
      list.scrollTop = list.scrollHeight;
    }
  }, [lastVisibleId]);

  return (
    <details className="protocol-viewer">
      <summary>Protocol log ({visible.length})</summary>
      <div className="protocol-viewer-controls">
        <label>
          <input type="checkbox" checked={showAudio} onChange={(e) => setShowAudio(e.target.checked)} />
          Show audio packets
        </label>
        <button type="button" onClick={onClear}>
          Clear
        </button>
      </div>
      <ol
        className="protocol-viewer-entries"
        ref={listRef}
        onScroll={(e) => {
          const list = e.currentTarget;
          // A few px of slack: scroll positions can be fractional.
          followingRef.current = list.scrollHeight - list.scrollTop - list.clientHeight < 4;
        }}
      >
        {visible.map((entry) => (
          <li key={entry.id} className={entry.ok ? undefined : "protocol-viewer-invalid"}>
            <details>
              <summary>
                <time>{entry.receivedAt.toLocaleTimeString()}</time> {entryLabel(entry)}
              </summary>
              <pre>{entry.ok ? JSON.stringify(entry.message.data, null, 2) : `${entry.error}\n\n${JSON.stringify(entry.raw, null, 2)}`}</pre>
            </details>
          </li>
        ))}
      </ol>
    </details>
  );
}
