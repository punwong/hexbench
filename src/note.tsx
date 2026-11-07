import "./shell";

import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import MarkdownIt from "markdown-it";
import { ChevronDown, ChevronUp, Eye, Pencil, Redo2, Undo2, X } from "lucide-react";
import { useTextHistory } from "./notehistory";
import { watchTheme } from "./theme";

/** 和后端 `notes.rs` 的 NoteFile / NoteSummary 对应。 */
type NoteFile = { id: string; title: string; body: string; modified: number };
type NoteSummary = { id: string; title: string; preview: string; images: unknown[]; modified: number };
type NotesChanged = { id: string | null; source: string | null };

const md = new MarkdownIt({ html: false, linkify: true, breaks: true });

// 正文里的 `images/xxx.png` 换成本地协议：原图从收藏目录直接给 WebView。
const fallbackImage = (tokens: any[], index: number, options: any, _env: unknown, self: any) =>
  self.renderToken(tokens, index, options);
const imageRule = md.renderer.rules.image ?? fallbackImage;
md.renderer.rules.image = (tokens, index, options, env, self) => {
  const token = tokens[index];
  const src = String(token.attrGet("src") ?? "");
  const match = /^\.?\/?images\/(.+)$/.exec(src);
  if (match) {
    token.attrSet("src", `http://hbnote.localhost/${encodeURI(match[1])}`);
  }
  token.attrSet("loading", "lazy");
  return imageRule(tokens, index, options, env, self);
};

const linkRule = md.renderer.rules.link_open ?? fallbackImage;
md.renderer.rules.link_open = (tokens, index, options, env, self) => {
  tokens[index].attrSet("rel", "noopener noreferrer");
  return linkRule(tokens, index, options, env, self);
};

type Mode = "preview" | "edit";
type Status = "saved" | "saving" | "error";

function ResizeHandles() {
  return (
    <>
      {(
        [
          ["n", "North", "ns-resize", "top:0;left:8px;right:8px;height:5px"],
          ["s", "South", "ns-resize", "bottom:0;left:8px;right:8px;height:5px"],
          ["w", "West", "ew-resize", "left:0;top:8px;bottom:8px;width:5px"],
          ["e", "East", "ew-resize", "right:0;top:8px;bottom:8px;width:5px"],
          ["nw", "NorthWest", "nwse-resize", "top:0;left:0;width:10px;height:10px"],
          ["ne", "NorthEast", "nesw-resize", "top:0;right:0;width:10px;height:10px"],
          ["sw", "SouthWest", "nesw-resize", "bottom:0;left:0;width:10px;height:10px"],
          ["se", "SouthEast", "nwse-resize", "bottom:0;right:0;width:10px;height:10px"],
        ] as const
      ).map(([key, direction, cursor, style]) => (
        <div
          key={key}
          style={{ position: "fixed", zIndex: 9999, cursor, ...cssFrom(style) }}
          onPointerDown={(event) => {
            event.preventDefault();
            event.stopPropagation();
            void getCurrentWindow().startResizeDragging(direction);
          }}
        />
      ))}
    </>
  );
}

function cssFrom(source: string): Record<string, string> {
  return Object.fromEntries(
    source
      .split(";")
      .filter(Boolean)
      .map((pair) => {
        const [name, value] = pair.split(":");
        return [name.replace(/-(\w)/g, (_, letter: string) => letter.toUpperCase()), value];
      }),
  );
}

function NoteApp() {
  const label = getCurrentWindow().label;
  const history = useTextHistory("");
  const bodyText = history.value;
  const [title, setTitle] = useState("");
  const [id, setId] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>("preview");
  const [status, setStatus] = useState<Status>("saved");
  const [ready, setReady] = useState(false);
  const [rolled, setRolled] = useState(false);
  const [error, setError] = useState("");

  const idRef = useRef<string | null>(null);
  const titleRef = useRef("");
  const bodyRef = useRef("");
  const synced = useRef({ title: "", body: "" });
  const chain = useRef<Promise<void>>(Promise.resolve());
  const bodyInputRef = useRef<HTMLTextAreaElement>(null);
  const rolledRef = useRef(false);
  const drag = useRef<{ x: number; y: number } | null>(null);

  titleRef.current = title;
  bodyRef.current = bodyText;

  const apply = useCallback(
    (file: NoteFile, initial: boolean) => {
      idRef.current = file.id;
      setId(file.id);
      titleRef.current = file.title;
      setTitle(file.title);
      bodyRef.current = file.body;
      history.reset(file.body);
      synced.current = { title: file.title, body: file.body };
      setStatus("saved");
      if (initial) {
        setMode(file.body.trim() ? "preview" : "edit");
      }
    },
    [history.reset],
  );

  const load = useCallback(
    async (initial: boolean) => {
      try {
        const file = await invoke<NoteFile>("note_state");
        apply(file, initial);
        setError("");
      } catch (problem) {
        if (initial) setError(String(problem));
      }
    },
    [apply],
  );

  // 首次加载：先读文件，画好了再让后端显示窗口（避免白屏闪一下）。
  useEffect(() => {
    watchTheme();
    let alive = true;
    void Promise.all([invoke<NoteFile>("note_state"), invoke<boolean>("note_rolled")])
      .then(([file, isRolled]) => {
        if (!alive) return;
        apply(file, true);
        rolledRef.current = isRolled;
        setRolled(isRolled);
        setReady(true);
        requestAnimationFrame(() => {
          void invoke("note_ready");
        });
      })
      .catch((problem) => {
        if (alive) setError(String(problem));
      });
    return () => {
      alive = false;
    };
  }, [apply]);

  // 保存：串行排队。慢一步的快照不能盖掉后写进去的那份。
  const save = useCallback(() => {
    const run = async () => {
      const currentId = idRef.current;
      if (!currentId) return;
      const snapshot = { title: titleRef.current, body: bodyRef.current };
      if (
        snapshot.title === synced.current.title &&
        snapshot.body === synced.current.body
      ) {
        return;
      }
      setStatus("saving");
      try {
        const saved = await invoke<NoteSummary>("notes_save", {
          id: currentId,
          title: snapshot.title.trim() || "未命名便签",
          body: snapshot.body,
          // 带上自己的标签：这条广播会传回来，页面据此忽略自己的回声，
          // 不然自动保存每 450ms 就会把正在输入的内容重读一遍。
          source: label,
        });
        if (saved.id !== idRef.current) {
          idRef.current = saved.id;
          setId(saved.id);
        }
        synced.current = snapshot;
        setStatus("saved");
      } catch (problem) {
        console.error("note: save failed", problem);
        setStatus("error");
      }
    };
    chain.current = chain.current.then(run, run);
    return chain.current;
  }, [label]);

  // 完全自动保存：停下来 450ms 就写盘，没有保存按钮。
  useEffect(() => {
    if (!ready || !id) return;
    if (title === synced.current.title && bodyText === synced.current.body) return;
    setStatus("saving");
    const timer = window.setTimeout(() => {
      void save();
    }, 450);
    return () => window.clearTimeout(timer);
  }, [ready, id, title, bodyText, save]);

  // 别处（呼出面板）改了这条笔记：没在输入时跟着刷新。
  useEffect(() => {
    let alive = true;
    const stop = listen<NotesChanged>("notes-changed", (event) => {
      if (!alive) return;
      const payload = event.payload;
      if (payload?.source === label) return;
      if (document.hasFocus()) return;
      void load(false);
    });
    return () => {
      alive = false;
      void stop.then((unlisten) => unlisten());
    };
  }, [label, load]);

  // 移动 / 改大小：防抖记下几何，下次启动原样摆回来。
  useEffect(() => {
    const win = getCurrentWindow();
    let timer = 0;
    let last = 0;
    const write = async () => {
      try {
        const [position, size] = await Promise.all([win.outerPosition(), win.innerSize()]);
        await invoke("note_geometry", {
          x: position.x,
          y: position.y,
          w: size.width,
          h: size.height,
        });
      } catch {
        // 窗口正在关掉时查不到几何，忽略。
      }
    };
    const schedule = () => {
      const now = Date.now();
      // 拖动的第一下立刻记一次，长距离拖动途中不写盘，松手后再记最终位置。
      if (now - last > 400) {
        last = now;
        void write();
      }
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        last = Date.now();
        void write();
      }, 400);
    };
    const moved = win.onMoved(schedule);
    const resized = win.onResized(schedule);
    return () => {
      window.clearTimeout(timer);
      void moved.then((unlisten) => unlisten());
      void resized.then((unlisten) => unlisten());
    };
  }, []);

  const rendered = useMemo(() => md.render(bodyText), [bodyText]);

  // 顶栏既能拖窗、又能双击收起：按下先不动，真的拖出去几像素才交给系统拖动，
  // 这样单击 / 双击不会被拖动吃掉。
  const barPointerDown = (event: React.PointerEvent) => {
    if (event.button !== 0) return;
    if ((event.target as HTMLElement).closest("button, input, textarea, a")) return;
    drag.current = { x: event.screenX, y: event.screenY };
  };

  const barPointerMove = (event: React.PointerEvent) => {
    const start = drag.current;
    if (!start) return;
    if (Math.abs(event.screenX - start.x) + Math.abs(event.screenY - start.y) < 4) return;
    drag.current = null;
    void invoke("begin_drag");
  };

  const barPointerUp = () => {
    drag.current = null;
  };

  const toggleRoll = (event?: React.MouseEvent) => {
    event?.stopPropagation();
    const next = !rolledRef.current;
    rolledRef.current = next;
    setRolled(next);
    void invoke("note_roll", { rolled: next }).catch((problem) => {
      console.error("note: roll failed", problem);
      rolledRef.current = !next;
      setRolled(!next);
    });
  };

  const onPreviewClick = (event: React.MouseEvent) => {
    const anchor = (event.target as HTMLElement).closest("a");
    if (!anchor) return;
    event.preventDefault();
    const href = anchor.getAttribute("href") ?? "";
    void invoke("open_url", { url: href }).catch((problem) => {
      console.error("note: open link failed", problem);
    });
  };

  const enterEdit = () => {
    setMode("edit");
    requestAnimationFrame(() => {
      const input = bodyInputRef.current;
      if (!input) return;
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    });
  };

  const unpin = () => {
    void invoke("note_unpin").catch((problem) => {
      console.error("note: unpin failed", problem);
    });
  };

  if (error) {
    return (
      <div className="note">
        <div className="bar" onPointerDown={barPointerDown}>
          <span className="dot" />
          <span className="bar-title">便签打不开</span>
        </div>
        <div className="empty">
          <span>{error}</span>
          <div style={{ display: "flex", gap: 8 }}>
            <button type="button" onClick={() => void load(true)}>
              重试
            </button>
            <button type="button" onClick={unpin}>
              关闭便签
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className={`note${rolled ? " rolled" : ""}`}>
      {!rolled && <ResizeHandles />}
      <header
        className="bar"
        onPointerDown={barPointerDown}
        onPointerMove={barPointerMove}
        onPointerUp={barPointerUp}
        onDoubleClick={(event) => {
          if ((event.target as HTMLElement).closest("button, input, textarea, a")) return;
          toggleRoll();
        }}
      >
        <span className="dot" title="已钉在桌面" />
        <span className="bar-title" title={title || "未命名便签"}>
          {title || "未命名便签"}
        </span>
        {!rolled && (
          <>
            <span className={`status${status === "error" ? " error" : ""}`}>
              {status === "saving" ? "保存中…" : status === "error" ? "保存失败" : "已保存"}
            </span>
            <button
              type="button"
              className="tool"
              title="撤销（Ctrl+Z）"
              disabled={!history.canUndo}
              onClick={history.undo}
            >
              <Undo2 size={14} strokeWidth={2} />
            </button>
            <button
              type="button"
              className="tool"
              title="重做（Ctrl+Y）"
              disabled={!history.canRedo}
              onClick={history.redo}
            >
              <Redo2 size={14} strokeWidth={2} />
            </button>
            <button
              type="button"
              className="tool"
              title={mode === "edit" ? "预览排版" : "编辑"}
              onClick={() => {
                if (mode === "edit") {
                  void save();
                  setMode("preview");
                } else {
                  enterEdit();
                }
              }}
            >
              {mode === "edit" ? (
                <Eye size={14} strokeWidth={2} />
              ) : (
                <Pencil size={14} strokeWidth={2} />
              )}
            </button>
          </>
        )}
        <button
          type="button"
          className="tool"
          title={rolled ? "展开（双击标题栏）" : "收起（双击标题栏）"}
          onClick={toggleRoll}
        >
          {rolled ? (
            <ChevronDown size={14} strokeWidth={2} />
          ) : (
            <ChevronUp size={14} strokeWidth={2} />
          )}
        </button>
        <button
          type="button"
          className="tool close"
          title="关闭便签（内容留在便签列表里）"
          aria-label="关闭便签"
          onClick={unpin}
        >
          <X size={14} strokeWidth={2} />
        </button>
      </header>

      {rolled ? null : mode === "edit" ? (
        <div className="editor">
          <input
            className="title-input"
            value={title}
            placeholder="标题"
            spellCheck={false}
            autoFocus={!bodyText.trim()}
            onChange={(event) => setTitle(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === "Enter") {
                event.preventDefault();
                bodyInputRef.current?.focus();
              }
            }}
          />
          <textarea
            ref={bodyInputRef}
            className="body-input"
            value={bodyText}
            placeholder="写点什么…支持 Markdown，拖图片进便签会自动插图。"
            spellCheck={false}
            autoFocus={!!bodyText.trim()}
            onChange={(event) => history.set(event.target.value)}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              const mod = event.ctrlKey || event.metaKey;
              if (!mod) return;
              const key = event.key.toLowerCase();
              if (key === "z") {
                event.preventDefault();
                if (event.shiftKey) history.redo();
                else history.undo();
              } else if (key === "y") {
                event.preventDefault();
                history.redo();
              }
            }}
          />
        </div>
      ) : bodyText.trim() ? (
        <div
          className="preview"
          onDoubleClick={enterEdit}
          onClick={onPreviewClick}
          title="双击编辑"
        >
          <div className="md" dangerouslySetInnerHTML={{ __html: rendered }} />
        </div>
      ) : (
        <div className="empty">
          <strong>这条便签还是空的</strong>
          写点什么，自动保存
          <button type="button" onClick={enterEdit}>
            开始编辑
          </button>
        </div>
      )}

      {!ready && !rolled && <div className="empty">正在打开…</div>}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <NoteApp />
  </React.StrictMode>,
);
