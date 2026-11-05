import { useCallback, useRef, useState } from "react";

/**
 * 笔记正文的撤销 / 重做。
 *
 * 编辑器是「完全自动保存」的，没有保存按钮，撤销就是唯一的退路。浏览器
 * textarea 自带的撤销是黑盒的（受控组件里被 React 重填一次就可能整段清掉），
 * 所以自己记：连续输入合并成一步（停顿 650ms 或超过一次提交才算新的一步），
 * 按钮和 Ctrl+Z / Ctrl+Y 都走这一份记录。
 */
export type TextHistory = {
  value: string;
  /** 用户输入：记一步历史，同时清掉重做。 */
  set: (next: string) => void;
  undo: () => void;
  redo: () => void;
  canUndo: boolean;
  canRedo: boolean;
  /** 从磁盘重新读进来：内容整个换掉，历史清空。 */
  reset: (value: string) => void;
};

/** 连续输入的合并窗口：停下来超过这么久，下一次输入才另起一步。 */
const MERGE_MS = 650;
const MAX_STEPS = 300;

export function useTextHistory(initial = ""): TextHistory {
  const [value, setValue] = useState(initial);
  const [depth, setDepth] = useState({ undo: 0, redo: 0 });
  const valueRef = useRef(initial);
  const past = useRef<string[]>([]);
  const future = useRef<string[]>([]);
  const lastPush = useRef(0);

  const sync = useCallback(() => {
    setDepth({ undo: past.current.length, redo: future.current.length });
  }, []);

  const set = useCallback(
    (next: string) => {
      if (next === valueRef.current) return;
      const now = Date.now();
      if (now - lastPush.current > MERGE_MS || past.current.length === 0) {
        past.current.push(valueRef.current);
        if (past.current.length > MAX_STEPS) past.current.shift();
      }
      lastPush.current = now;
      future.current = [];
      valueRef.current = next;
      setValue(next);
      sync();
    },
    [sync],
  );

  const undo = useCallback(() => {
    const previous = past.current.pop();
    if (previous === undefined) return;
    future.current.push(valueRef.current);
    valueRef.current = previous;
    lastPush.current = 0;
    setValue(previous);
    sync();
  }, [sync]);

  const redo = useCallback(() => {
    const next = future.current.pop();
    if (next === undefined) return;
    past.current.push(valueRef.current);
    valueRef.current = next;
    lastPush.current = 0;
    setValue(next);
    sync();
  }, [sync]);

  const reset = useCallback(
    (next: string) => {
      past.current = [];
      future.current = [];
      lastPush.current = 0;
      valueRef.current = next;
      setValue(next);
      sync();
    },
    [sync],
  );

  return {
    value,
    set,
    undo,
    redo,
    canUndo: depth.undo > 0,
    canRedo: depth.redo > 0,
    reset,
  };
}
