import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";

interface Phrase {
  id: string;
  title: string;
  text: string;
}

interface PhrasesUi {
  collapsed: boolean;
  target: string;
  sendOnClick: boolean;
}

/// The hexagon is the panel's "body": it shrinks to the icon and grows into the
/// list, so the same graphic is always the thing you click.
function LogoIcon({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <defs>
        <linearGradient id="hb-logo" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#3b82f6" />
          <stop offset="100%" stopColor="#8b5cf6" />
        </linearGradient>
      </defs>
      <path d="M12 2 20.6 7v10L12 22 3.4 17V7z" fill="url(#hb-logo)" />
      <path
        d="M12 8.6 15.4 10.5v3.8L12 16.2 8.6 14.3v-3.8z"
        fill="#ffffff"
        fillOpacity="0.9"
      />
    </svg>
  );
}

function Panel() {
  const [items, setItems] = useState<Phrase[]>([]);
  const [editing, setEditing] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const [sendOnClick, setSendOnClick] = useState(false);
  const [toast, setToast] = useState("");
  const dirty = useRef(false);
  const saveTimer = useRef<number | undefined>(undefined);
  const pressAt = useRef<{ x: number; y: number } | null>(null);

  const flash = useCallback((text: string) => {
    setToast(text);
    window.setTimeout(() => setToast(""), 1400);
  }, []);

  useEffect(() => {
    void invoke<PhrasesUi>("phrases_ui")
      .then((ui) => {
        setCollapsed(ui.collapsed);
        setSendOnClick(ui.sendOnClick);
      })
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    // The main window can change the mode too; pick it up without a reload.
    const timer = window.setInterval(() => {
      void invoke<PhrasesUi>("phrases_ui")
        .then((ui) => setSendOnClick(ui.sendOnClick))
        .catch(() => undefined);
    }, 2000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const load = () => {
      // Never clobber what the user is typing.
      if (dirty.current) return;
      void invoke<Phrase[]>("phrase_items")
        .then(setItems)
        .catch(() => undefined);
    };
    load();
    const timer = window.setInterval(load, 1500);
    return () => window.clearInterval(timer);
  }, []);

  const persist = useCallback(
    (next: Phrase[]) => {
      dirty.current = true;
      window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => {
        void invoke("phrase_save", { items: next })
          .catch((reason) => flash(String(reason)))
          .finally(() => {
            dirty.current = false;
          });
      }, 400);
    },
    [flash],
  );

  const update = useCallback(
    (next: Phrase[]) => {
      setItems(next);
      persist(next);
    },
    [persist],
  );

  const send = async (phrase: Phrase) => {
    if (editing) return;
    try {
      await invoke("phrase_send", { text: phrase.text });
      if (sendOnClick) {
        // Let the text land before the Enter; Chromium inputs are async.
        await new Promise((resolve) => window.setTimeout(resolve, 60));
        await invoke("phrase_submit");
        flash("已发送");
      } else {
        flash("已填入");
      }
    } catch (reason) {
      flash(String(reason));
    }
  };

  const toggleSend = () => {
    const next = !sendOnClick;
    setSendOnClick(next);
    void invoke("phrase_send_mode", { send: next }).catch(() => undefined);
  };

  const toggleEditing = () => {
    const next = !editing;
    setEditing(next);
    void invoke("panel_editing", { editing: next }).catch(() => undefined);
  };

  const setPanelCollapsed = (value: boolean) => {
    setCollapsed(value);
    void invoke("panel_collapse", { collapsed: value }).catch(() => undefined);
  };

  const startDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.closest("button, input, textarea, .head-actions")) return;
    pressAt.current = { x: event.clientX, y: event.clientY };
    void invoke("panel_drag").catch(() => undefined);
  };

  const expandIfStill = (event: ReactPointerEvent<HTMLDivElement>) => {
    const start = pressAt.current;
    pressAt.current = null;
    if (!start) return;
    const moved =
      Math.abs(event.clientX - start.x) + Math.abs(event.clientY - start.y) > 4;
    if (!moved) setPanelCollapsed(false);
  };

  const addPhrase = () => {
    const phrased: Phrase = { id: `p${Date.now()}`, title: "新话术", text: "" };
    if (!editing) {
      setEditing(true);
      void invoke("panel_editing", { editing: true }).catch(() => undefined);
    }
    update([...items, phrased]);
    window.setTimeout(() => {
      const nodes = document.querySelectorAll<HTMLElement>(".edit-item textarea");
      nodes[nodes.length - 1]?.focus();
    }, 60);
  };

  if (collapsed) {
    return (
      <div className="panel collapsed">
        <div
          className="bubble"
          role="button"
          tabIndex={0}
          title="展开快捷语"
          onPointerDown={startDrag}
          onPointerUp={expandIfStill}
        >
          <LogoIcon size={30} />
          <span className="count">{items.length}</span>
        </div>
      </div>
    );
  }

  return (
    <div className="panel">
      <div className="head" onPointerDown={startDrag}>
        <button
          type="button"
          className="head-logo"
          title="收起成图标"
          onClick={() => setPanelCollapsed(true)}
        >
          <LogoIcon size={18} />
        </button>
        <span className="head-title">快捷语</span>
        <span className="head-actions">
          <button
            type="button"
            className={sendOnClick ? "icon active" : "icon"}
            title={sendOnClick ? "点击即发送（已开）" : "点击只填入"}
            onClick={toggleSend}
          >
            ↵
          </button>
          <button type="button" className="icon" title="添加话术" onClick={addPhrase}>
            ＋
          </button>
          <button
            type="button"
            className="icon"
            title={editing ? "完成编辑" : "编辑话术"}
            onClick={toggleEditing}
          >
            {editing ? "✓" : "✎"}
          </button>
        </span>
      </div>

      <div className="list">
        {items.length === 0 && (
          <div className="empty">
            还没有话术，点右上角 ＋ 添加。
            <br />
            点一个就发到当前输入框。
          </div>
        )}

        {items.map((item) =>
          editing ? (
            <div className="edit-item" key={item.id}>
              <input
                value={item.title}
                placeholder="标题"
                onChange={(event) =>
                  update(
                    items.map((entry) =>
                      entry.id === item.id ? { ...entry, title: event.target.value } : entry,
                    ),
                  )
                }
              />
              <textarea
                rows={2}
                value={item.text}
                placeholder="要发送的内容"
                onChange={(event) =>
                  update(
                    items.map((entry) =>
                      entry.id === item.id ? { ...entry, text: event.target.value } : entry,
                    ),
                  )
                }
              />
              <button
                type="button"
                className="del"
                onClick={() => update(items.filter((entry) => entry.id !== item.id))}
              >
                删除
              </button>
            </div>
          ) : (
            <button
              key={item.id}
              type="button"
              className="item"
              onClick={() => void send(item)}
            >
              <span className="title">{item.title}</span>
              <span className="preview">{item.text}</span>
            </button>
          ),
        )}
      </div>

      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}

createRoot(document.getElementById("root")!).render(<Panel />);
