/* Orrery UI kit — overlays: command palette, detail drawer, settings. */
const { useState: useStateO, useEffect: useEffectO, useRef: useRefO } = React;

function CommandPalette({ repos, onClose, onPick, onCommand }) {
  const [q, setQ] = useStateO('');
  const [sel, setSel] = useStateO(0);
  const inputRef = useRefO(null);
  useEffectO(() => { inputRef.current && inputRef.current.focus(); }, []);

  const commands = [
    { id: 'feed', label: 'Open feed', icon: 'sparkles' },
    { id: 'grid', label: 'Open mission control', icon: 'layout-grid' },
    { id: 'rescan', label: 'Rescan all roots', icon: 'refresh-cw', kbd: '⌘R' },
    { id: 'add', label: 'Add root directory…', icon: 'folder-git-2' },
    { id: 'settings', label: 'Open settings', icon: 'settings', kbd: '⌘,' },
    { id: 'list', label: 'Toggle list view', icon: 'list' },
  ];
  const ql = q.toLowerCase();
  const fCmds = commands.filter(c => c.label.toLowerCase().includes(ql));
  const fRepos = repos.filter(r => r.name.toLowerCase().includes(ql) || r.slug.toLowerCase().includes(ql));
  const flat = [...fCmds.map(c => ({ t: 'cmd', ...c })), ...fRepos.map(r => ({ t: 'repo', r }))];

  useEffectO(() => {
    const h = (e) => {
      if (e.key === 'ArrowDown') { e.preventDefault(); setSel(s => Math.min(s + 1, flat.length - 1)); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); setSel(s => Math.max(s - 1, 0)); }
      else if (e.key === 'Enter') {
        const it = flat[sel];
        if (!it) return;
        if (it.t === 'cmd') onCommand(it.id); else onPick(it.r);
      } else if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, [flat, sel]);

  let idx = -1;
  return (
    <div className="scrim" onClick={onClose}>
      <div className="cmdk" onClick={e => e.stopPropagation()}>
        <div className="cmdk-in">
          <Icon name="search" size={18} className="ico" />
          <input ref={inputRef} value={q} onChange={e => { setQ(e.target.value); setSel(0); }}
            placeholder="Search repos, run a command…" />
        </div>
        <div className="cmdk-list">
          {fCmds.length > 0 && <div className="cmdk-grp">Commands</div>}
          {fCmds.map(c => { idx++; const i = idx; return (
            <div key={c.id} className={'cmdk-item' + (sel === i ? ' sel' : '')}
              onMouseEnter={() => setSel(i)} onClick={() => onCommand(c.id)}>
              <Icon name={c.icon} size={16} className="ico" />{c.label}
              {c.kbd && <span className="sp">{c.kbd}</span>}
            </div>
          ); })}
          {fRepos.length > 0 && <div className="cmdk-grp">Repos</div>}
          {fRepos.map(r => { idx++; const i = idx; return (
            <div key={r.slug} className={'cmdk-item' + (sel === i ? ' sel' : '')}
              onMouseEnter={() => setSel(i)} onClick={() => onPick(r)}>
              <span className="dot" style={{ background: r.langColor }}></span>{r.name}
              <span className="sp">{r.path}</span>
            </div>
          ); })}
          {flat.length === 0 && <div className="cmdk-item" style={{ color: 'var(--fg-3)' }}>No matches</div>}
        </div>
        <div className="cmdk-foot">
          <span><span className="k">↑↓</span>navigate</span>
          <span><span className="k">↵</span>open</span>
          <span><span className="k">esc</span>close</span>
        </div>
      </div>
    </div>
  );
}

function RepoDetail({ r, onClose, onIDE, onAgent }) {
  const statusLabel = { clean: 'Clean', dirty: `${r.dirty} uncommitted`, behind: `${r.behind} behind`, stale: 'Stale' }[r.status];
  return (
    <div className="scrim" onClick={onClose}>
      <div className="drawer" onClick={e => e.stopPropagation()}>
        <div className="dr-head">
          <button className="dr-close" onClick={onClose}><Icon name="x" size={16} /></button>
          <div className="dr-name">
            <span className="ldot" style={{ background: r.langColor, color: r.langColor }}></span>{r.name}
          </div>
          <div className="dr-slug">{r.slug} · {r.path}</div>
        </div>
        <div className="dr-body">
          <div className="dr-ai">
            <div className="h"><Icon name="sparkles" size={14} className="ico" />Local AI summary</div>
            <p>{r.ai}</p>
          </div>

          <div className="dr-sec">
            <div className="h">Git state</div>
            <div className="statgrid">
              <div className="statcell"><div className="k">Branch</div><div className="v"><Icon name="git-branch" size={14} className="ico" style={{ color: 'var(--fg-2)' }} />{r.branch}</div></div>
              <div className="statcell"><div className="k">Working tree</div><div className="v" style={{ color: r.dirty ? 'var(--dirty)' : 'var(--clean)' }}><Icon name={r.dirty ? 'circle-dot' : 'check'} size={14} className="ico" />{statusLabel}</div></div>
              <div className="statcell"><div className="k">Ahead / behind</div><div className="v">↑{r.ahead} ↓{r.behind}</div></div>
              <div className="statcell"><div className="k">Last commit</div><div className="v"><Icon name="clock" size={14} className="ico" style={{ color: 'var(--fg-2)' }} />{r.commitAgo}</div></div>
            </div>
          </div>

          <div className="dr-sec">
            <div className="h">Host · {r.host}</div>
            <div className="statgrid">
              <div className="statcell"><div className="k">Stars</div><div className="v" style={{ color: 'var(--star)' }}><Icon name="star" size={14} className="ico" />{r.stars}</div></div>
              <div className="statcell"><div className="k">Open issues</div><div className="v">{r.issues}</div></div>
              <div className="statcell"><div className="k">Latest release</div><div className="v"><Icon name="tag" size={14} className="ico" style={{ color: 'var(--fg-2)' }} />{r.release || '—'}</div></div>
              <div className="statcell"><div className="k">Language</div><div className="v"><span className="ldot" style={{ width: 9, height: 9, borderRadius: '50%', background: r.langColor, display: 'inline-block' }}></span>{r.lang}</div></div>
            </div>
          </div>

          <div className="dr-sec">
            <div className="h">Recent commits</div>
            <div className="commits">
              {r.commits.map((c, i) => (
                <div className="commit" key={i}>
                  <Icon name="git-commit-horizontal" size={14} className="ico" />
                  <div><div className="msg">{c.msg}</div><div className="meta">{c.meta}</div></div>
                </div>
              ))}
            </div>
          </div>
        </div>
        <div className="dr-acts">
          <button className="cbtn ide" onClick={() => onIDE(r)}><Icon name="code" size={14} className="ico" />Open in IDE</button>
          <button className="cbtn agent" onClick={() => onAgent(r)}><Icon name="square-terminal" size={14} className="ico" />Drop Agent</button>
        </div>
      </div>
    </div>
  );
}

function Settings({ onClose }) {
  const [live, setLive] = useStateO(true);
  const [ai, setAi] = useStateO(true);
  const [grain, setGrain] = useStateO(true);
  const rows = [
    { t: 'IDE command', d: 'Template run when you click “Open in IDE”.', val: 'code {path}' },
    { t: 'Agent command', d: 'Drops a terminal coding agent into the repo.', val: 'kitty -d {path} -e claude' },
    { t: 'Scan depth', d: 'How deep to walk each root looking for .git.', val: '3' },
  ];
  const toggles = [
    { t: 'Live watch (inotify)', d: 'Refresh cards automatically on file changes.', v: live, set: setLive },
    { t: 'Local AI summaries', d: 'Generate per-repo blurbs on-device with llama.cpp.', v: ai, set: setAi },
    { t: 'Star-field background', d: 'Subtle ambient texture behind the grid.', v: grain, set: setGrain },
  ];
  return (
    <div className="scrim" onClick={onClose}>
      <div className="settings" onClick={e => e.stopPropagation()}>
        <div className="set-head">
          <h2>Settings</h2>
          <button className="dr-close" style={{ position: 'static' }} onClick={onClose}><Icon name="x" size={16} /></button>
        </div>
        <div className="set-body">
          {rows.map(r => (
            <div className="set-row" key={r.t}>
              <div className="meta"><div className="t">{r.t}</div><div className="d">{r.d}</div></div>
              <span className="val">{r.val}</span>
            </div>
          ))}
          {toggles.map(r => (
            <div className="set-row" key={r.t}>
              <div className="meta"><div className="t">{r.t}</div><div className="d">{r.d}</div></div>
              <div className={'toggle' + (r.v ? ' on' : '')} onClick={() => r.set(!r.v)}><i></i></div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { CommandPalette, RepoDetail, Settings });
