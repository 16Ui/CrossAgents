// Lightweight remote/centre directory browser modal so users can visually pick a folder on
// another machine (e.g. Windows browsing MacBook-Pro, or Mac browsing the Windows centre) without
// having to memorize and type full paths.

import { useEffect, useMemo, useState, type FormEvent } from "react";
import { ArrowUp, Folder, Home, RefreshCw, X } from "lucide-react";
import type { NodeDirsResponse } from "@bindings/NodeDirsResponse";
import { api, errorText } from "./api";
import { ErrorBox } from "./components";
import { normalizeNodeOs } from "./nodes";
import { OsIcon } from "./osIcon";

export function RemoteDirPicker({
  node,
  label,
  os,
  initialPath = "",
  onSelect,
  onClose,
}: {
  /** `"local"` for the hub machine, or a remote node id / name. */
  node: string;
  label: string;
  os?: string;
  initialPath?: string;
  onSelect: (path: string) => void;
  onClose: () => void;
}) {
  const [data, setData] = useState<NodeDirsResponse>();
  const [pathInput, setPathInput] = useState(initialPath);
  const [filter, setFilter] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();

  const load = (targetPath: string) => {
    setLoading(true);
    setError(undefined);
    api.nodeDirs(node, targetPath).then(
      (res) => {
        setData(res);
        setPathInput(res.path);
        setFilter("");
        setLoading(false);
      },
      (err) => {
        setError(errorText(err));
        setLoading(false);
      },
    );
  };

  useEffect(() => {
    load(initialPath);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [node]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const visibleDirs = useMemo(() => {
    const dirs = data?.dirs ?? [];
    const q = filter.trim().toLowerCase();
    if (!q) return dirs;
    return dirs.filter((d) => d.name.toLowerCase().includes(q));
  }, [data, filter]);

  const jump = (e: FormEvent) => {
    e.preventDefault();
    load(pathInput.trim());
  };

  const resolvedOs = normalizeNodeOs(data?.os ?? os);
  const canPick = !!data?.path;

  return (
    <div className="dir-picker-backdrop" role="dialog" aria-modal="true" aria-label={`浏览 ${label} 目录`} onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="dir-picker">
        <div className="dir-picker-head">
          <span className="dir-picker-title">
            <OsIcon os={resolvedOs} size={15} />
            浏览 {label} 上的目录
          </span>
          <button type="button" className="ghost icon small" onClick={onClose} aria-label="关闭">
            <X size={15} />
          </button>
        </div>
        <form className="dir-picker-nav" onSubmit={jump}>
          <button
            type="button"
            className="ghost icon small"
            onClick={() => data?.parent !== undefined && data.parent !== null && load(data.parent)}
            disabled={loading || !data?.parent}
            title="上一级目录"
            aria-label="上一级目录"
          >
            <ArrowUp size={14} />
          </button>
          <button
            type="button"
            className="ghost icon small"
            onClick={() => load("")}
            disabled={loading}
            title="回到主目录"
            aria-label="回到主目录"
          >
            <Home size={14} />
          </button>
          <input
            className="dir-picker-path mono"
            value={pathInput}
            onChange={(e) => setPathInput(e.target.value)}
            placeholder={resolvedOs === "windows" ? "例如 C:\\Users\\..." : "例如 /Users/..."}
            aria-label="目录路径"
          />
          <button type="submit" className="ghost small" disabled={loading}>
            进入
          </button>
          <button
            type="button"
            className="ghost icon small"
            onClick={() => load(data?.path ?? pathInput)}
            disabled={loading}
            title="刷新"
            aria-label="刷新"
          >
            <RefreshCw size={13} />
          </button>
        </form>
        <div className="dir-picker-filter">
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="筛选当前目录下的文件夹…"
            aria-label="筛选文件夹"
          />
        </div>
        {error && <div className="dir-picker-err"><ErrorBox error={error} /></div>}
        <div className="dir-picker-list" role="listbox" aria-label="子目录列表">
          {loading && !data ? (
            <div className="dir-picker-empty">正在读取 {label} 的目录…</div>
          ) : visibleDirs.length === 0 ? (
            <div className="dir-picker-empty">
              {filter.trim() ? "没有匹配的文件夹" : "这个目录下没有子文件夹"}
            </div>
          ) : (
            visibleDirs.map((dir) => (
              <button
                key={dir.path}
                type="button"
                className="dir-picker-row"
                onClick={() => load(dir.path)}
                title={dir.path}
              >
                <Folder size={14} />
                <span className="dir-picker-name">{dir.name}</span>
              </button>
            ))
          )}
          {data?.truncated && (
            <div className="dir-picker-empty small">仅显示前 500 个文件夹，可直接在上栏输入完整路径</div>
          )}
        </div>
        <div className="dir-picker-foot">
          <span className="dir-picker-current mono" title={data?.path || pathInput}>
            {data?.path || "请选择磁盘或目录"}
          </span>
          <div className="row">
            <button type="button" className="ghost small" onClick={onClose}>
              取消
            </button>
            <button
              type="button"
              className="primary small"
              disabled={!canPick || loading}
              onClick={() => data?.path && onSelect(data.path)}
            >
              选择此目录
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
