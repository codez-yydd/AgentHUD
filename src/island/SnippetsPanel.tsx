/**
 * 灵动岛「片段库」子页：常用 prompt / 模板的本机管理。
 *
 * - 列表：点一条即复制正文（不弹编辑器，符合"随手用"）；名称留空的由后端
 *   取正文首行。
 * - 编辑：名称可选、正文必填；保存后重新拉列表（后端按 updated_at 倒序）。
 * - 删除：两段式确认（第一次点变"再点删除"，2.4s 后复位），避免误删。
 * 数据在 ~/.agenthud/snippets.json，纯本机，不联网。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { copyText, deleteSnippet, listSnippets, saveSnippet } from "./api";
import type { Snippet } from "./types";
import { Icon } from "./ui";

const PENDING_RESET_MS = 2400;

function fmtWhen(ts: number): string {
  const diff = Math.max(0, Date.now() - ts);
  if (diff < 60_000) return "刚刚";
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟前`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时前`;
  const date = new Date(ts);
  return `${date.getMonth() + 1}/${date.getDate()}`;
}

export function SnippetsPanel({ onToast }: { onToast: (message: string) => void }) {
  const [items, setItems] = useState<Snippet[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  /** null = 不在编辑；id 为 null 表示新建 */
  const [editing, setEditing] = useState<{ id: string | null; name: string; body: string } | null>(
    null
  );
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [pendingDelete, setPendingDelete] = useState("");
  const deleteTimer = useRef<number | null>(null);

  const reload = () => {
    return listSnippets()
      .then((list) => {
        setItems(list);
        setError("");
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  };

  useEffect(() => {
    void reload();
    return () => {
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
    };
  }, []);

  const keyword = query.trim().toLowerCase();
  const visible = useMemo(
    () =>
      keyword
        ? items.filter(
            (item) =>
              item.name.toLowerCase().includes(keyword) ||
              item.body.toLowerCase().includes(keyword)
          )
        : items,
    [items, keyword]
  );

  const useSnippet = (item: Snippet) => {
    void copyText(item.body)
      .then(() => onToast(`已复制「${item.name}」`))
      .catch((e) => onToast(String(e)));
  };

  const openEditor = (item?: Snippet) => {
    setError("");
    setPendingDelete("");
    setEditing(
      item
        ? { id: item.id, name: item.name, body: item.body }
        : { id: null, name: "", body: "" }
    );
  };

  const commit = () => {
    if (!editing || saving) return;
    setSaving(true);
    void saveSnippet(editing.id, editing.name, editing.body)
      .then((saved) => {
        setEditing(null);
        onToast(`已保存「${saved.name}」`);
        return reload();
      })
      .catch((e) => setError(String(e)))
      .finally(() => setSaving(false));
  };

  const remove = (item: Snippet) => {
    if (pendingDelete !== item.id) {
      setPendingDelete(item.id);
      onToast("再点一次删除这个片段");
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      deleteTimer.current = window.setTimeout(() => setPendingDelete(""), PENDING_RESET_MS);
      return;
    }
    setPendingDelete("");
    void deleteSnippet(item.id)
      .then(() => {
        onToast("片段已删除");
        if (editing?.id === item.id) setEditing(null);
        return reload();
      })
      .catch((e) => setError(String(e)));
  };

  // ===== 编辑态 =====
  if (editing) {
    return (
      <div className="snip-page">
        <div className="snip-head">
          <span className="snip-title">{editing.id ? "编辑片段" : "新建片段"}</span>
          <span className="tool-hint">名称可留空，会自动取正文第一行</span>
        </div>
        <input
          className="snip-input"
          value={editing.name}
          placeholder="名称（可留空）"
          maxLength={80}
          onChange={(event) =>
            setEditing((current) =>
              current ? { ...current, name: event.target.value } : current
            )
          }
        />
        <textarea
          className="snip-textarea"
          rows={8}
          value={editing.body}
          placeholder="片段内容，例如一段审查指令或提交信息模板"
          spellCheck={false}
          onChange={(event) =>
            setEditing((current) =>
              current ? { ...current, body: event.target.value } : current
            )
          }
        />
        <div className="snip-editor-foot">
          <span className="snip-count">{editing.body.length} 字符</span>
          <div className="snip-editor-actions">
            <button
              type="button"
              className="tool-btn"
              onClick={() => {
                setEditing(null);
                setError("");
              }}
            >
              取消
            </button>
            <button
              type="button"
              className="tool-btn primary"
              disabled={saving || !editing.body.trim()}
              onClick={commit}
            >
              {saving ? "保存中…" : "保存"}
            </button>
          </div>
        </div>
        {error ? <div className="tool-error">{error}</div> : null}
      </div>
    );
  }

  // ===== 列表态 =====
  return (
    <div className="snip-page">
      <div className="snip-head">
        <span className="tool-hint">
          {items.length} 条 · 点一条即复制 · 存于 ~/.agenthud/snippets.json
        </span>
        <button type="button" className="mini-text-btn" onClick={() => openEditor()}>
          新建
        </button>
      </div>

      {items.length > 0 ? (
        <label className="clip-search">
          <Icon name="search" size={11} />
          <input
            value={query}
            placeholder="搜索名称或内容"
            onChange={(event) => setQuery(event.target.value)}
          />
          {query ? (
            <button
              type="button"
              className="clip-search-clear"
              title="清空搜索"
              aria-label="清空搜索"
              onClick={() => setQuery("")}
            >
              ×
            </button>
          ) : null}
        </label>
      ) : null}

      {error ? <div className="tool-error">{error}</div> : null}

      {loading ? (
        <div className="tool-empty">读取中…</div>
      ) : items.length === 0 ? (
        <div className="tool-empty">
          还没有片段
          <br />
          把常用的 prompt 存进来，以后一键复制
        </div>
      ) : visible.length === 0 ? (
        <div className="tool-empty">没有匹配的片段</div>
      ) : (
        <div className="snip-list">
          {visible.map((item) => (
            <div
              className="snip-item"
              key={item.id}
              role="button"
              tabIndex={0}
              onClick={() => useSnippet(item)}
              onKeyDown={(event) => {
                if (event.key !== "Enter" && event.key !== " ") return;
                event.preventDefault();
                useSnippet(item);
              }}
            >
              <div className="snip-main">
                <span className="snip-name">{item.name}</span>
                <span className="snip-body">{item.body}</span>
                <span className="snip-meta">更新于 {fmtWhen(item.updated_at)}</span>
              </div>
              <div className="snip-actions">
                <button
                  type="button"
                  className="clip-action"
                  title="编辑"
                  aria-label="编辑"
                  onClick={(event) => {
                    event.stopPropagation();
                    openEditor(item);
                  }}
                >
                  <Icon name="edit" size={12} />
                </button>
                <button
                  type="button"
                  className={`clip-action danger ${pendingDelete === item.id ? "on" : ""}`}
                  title={pendingDelete === item.id ? "再点一次删除" : "删除"}
                  aria-label="删除"
                  onClick={(event) => {
                    event.stopPropagation();
                    remove(item);
                  }}
                >
                  <Icon name="trash" size={12} />
                </button>
                <span className="clip-copy" aria-hidden>
                  <Icon name="copy" size={13} />
                </span>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
