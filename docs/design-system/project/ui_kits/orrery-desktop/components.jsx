/* Orrery UI kit — core components. Icon comes from icons.jsx (window). */
const { useState } = React;

function TitleBar({ roots, onSearch, onSettings, onRefresh, view }) {
  return (
    <div className="titlebar">
      <div className="tb-brand">
        <Icon name="orbit" size={24} className="tb-mark" />
        <span className="tb-word">Orrery</span>
      </div>
      <div className="tb-roots">
        <Icon name="folder" size={13} />
        <span>{roots.length} roots · {roots.reduce((a, r) => a + r.count, 0)} repos</span>
      </div>
      <div className="tb-spacer"></div>
      <div className="tb-search" onClick={onSearch}>
        <Icon name="search" size={15} style={{ color: 'var(--fg-3)' }} />
        <span className="ph">Search repos, run a command…</span>
        <span className="kbd">⌘K</span>
      </div>
      <button className="tb-iconbtn" title="Rescan" onClick={onRefresh}><Icon name="refresh-cw" size={17} /></button>
      <button className={'tb-iconbtn' + (view === 'settings' ? ' active' : '')} title="Settings" onClick={onSettings}><Icon name="settings" size={17} /></button>
      <div className="winctls">
        <button className="wc min" aria-label="minimize"></button>
        <button className="wc max" aria-label="maximize"></button>
        <button className="wc close" aria-label="close"></button>
      </div>
    </div>
  );
}

function Sidebar({ roots, repos, activeRoot, setActiveRoot, langFilter, setLangFilter, mode, setMode }) {
  const langs = [...new Set(repos.map(r => r.lang))].map(l => ({
    lang: l, color: repos.find(r => r.lang === l).langColor,
    count: repos.filter(r => r.lang === l).length,
  })).sort((a, b) => b.count - a.count);

  return (
    <div className="sidebar">
      <div className="sb-sec">
        <div className={'sb-item' + (mode === 'grid' ? ' active' : '')} onClick={() => setMode('grid')}>
          <Icon name="layout-grid" size={16} className="ico" /> Mission Control
        </div>
        <div className={'sb-item' + (mode === 'feed' ? ' active' : '')} onClick={() => setMode('feed')}>
          <Icon name="sparkles" size={16} className="ico" /> Feed
        </div>
      </div>

      {mode === 'grid' && (
      <React.Fragment>
      <div className="sb-sec">
        <div className="sb-lead">Roots <span className="add"><Icon name="plus" size={14} /></span></div>
        <div className={'sb-item' + (activeRoot === 'all' ? ' active' : '')} onClick={() => setActiveRoot('all')}>
          <Icon name="folder" size={16} className="ico" /> All repos
          <span className="count">{repos.length}</span>
        </div>
        {roots.map(r => (
          <div key={r.id} className={'sb-item' + (activeRoot === r.id ? ' active' : '')} onClick={() => setActiveRoot(r.id)}>
            <Icon name="folder-git-2" size={16} className="ico" />
            <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{r.path}</span>
            <span className="count">{r.count}</span>
          </div>
        ))}
      </div>

      <div className="sb-sec">
        <div className="sb-lead">Languages</div>
        {langs.map(l => (
          <div key={l.lang} className={'sb-item' + (langFilter === l.lang ? ' active' : '')}
            onClick={() => setLangFilter(langFilter === l.lang ? null : l.lang)}>
            <span className="dot" style={{ background: l.color, boxShadow: `0 0 7px ${l.color}` }}></span>
            {l.lang}<span className="count">{l.count}</span>
          </div>
        ))}
      </div>
      </React.Fragment>
      )}

      <div className="sb-foot">
        <Icon name="hard-drive" size={13} /> Cache synced · 2m ago
      </div>
    </div>
  );
}

function StatusRow({ r }) {
  return (
    <div className="card-status">
      <span className="st muted"><Icon name="git-branch" size={13} className="ico" />{r.branch}</span>
      {(r.ahead > 0 || r.behind > 0) && (
        <span className={'st ' + (r.behind > 0 ? 'behind' : 'clean')}>
          <Icon name="arrow-up" size={12} className="ico" />{r.ahead}
          <Icon name="arrow-down" size={12} className="ico" style={{ marginLeft: 4 }} />{r.behind}
        </span>
      )}
      {r.dirty > 0
        ? <span className="st dirty"><Icon name="circle-dot" size={13} className="ico" />{r.dirty}</span>
        : <span className="st clean"><Icon name="check" size={13} className="ico" />clean</span>}
    </div>
  );
}

function RepoCard({ r, view, onOpen, onFav, onIDE, onAgent }) {
  const stale = r.stale;
  return (
    <div className="card" onClick={() => onOpen(r)}>
      <div className="card-head">
        <div className="card-name">
          <span className="ldot" style={{ background: r.langColor, color: r.langColor }}></span>
          <span className="nm">{r.name}</span>
        </div>
        {view === 'grid' && (
          <Icon name="star" size={16} className={'card-fav' + (r.fav ? ' on' : '')}
            onClick={(e) => { e.stopPropagation(); onFav(r); }} />
        )}
        {view === 'list' && <span className="card-badge">{r.lang}</span>}
      </div>

      {view === 'grid' ? (
        <React.Fragment>
          <div className="card-slug">{r.slug} · {r.path}</div>
          <div className="card-desc">{r.desc}</div>
          {r.ai && <div className="card-ai"><Icon name="sparkles" size={12} className="ico" />{stale ? 'Dormant' : 'AI summary ready'}</div>}
          <StatusRow r={r} />
          <div className="card-host">
            <span className="st"><Icon name="star" size={13} className="ico" />{r.stars >= 1000 ? (r.stars / 1000).toFixed(1) + 'k' : r.stars}</span>
            <span className="st"><Icon name="clock" size={13} className="ico" style={{ color: 'var(--fg-3)' }} />{r.commitAgo}</span>
            <span className="st"><Icon name={r.host} size={13} className="ico" style={{ color: 'var(--fg-3)' }} /></span>
          </div>
          <div className="card-acts">
            <button className="cbtn ide" onClick={(e) => { e.stopPropagation(); onIDE(r); }}><Icon name="code" size={14} className="ico" />Open in IDE</button>
            <button className="cbtn agent" onClick={(e) => { e.stopPropagation(); onAgent(r); }}><Icon name="square-terminal" size={14} className="ico" />Agent</button>
          </div>
        </React.Fragment>
      ) : (
        <React.Fragment>
          <div className="l-desc">{r.desc}</div>
          <StatusRow r={r} />
          <span className="st muted" style={{ font: 'var(--text-data-sm)', color: 'var(--fg-3)' }}>{r.commitAgo}</span>
          <div className="card-acts">
            <button className="cbtn ide" onClick={(e) => { e.stopPropagation(); onIDE(r); }}><Icon name="code" size={14} className="ico" />IDE</button>
            <button className="cbtn agent" onClick={(e) => { e.stopPropagation(); onAgent(r); }}><Icon name="square-terminal" size={14} className="ico" />Agent</button>
          </div>
        </React.Fragment>
      )}
    </div>
  );
}

function EmptyState({ onAdd }) {
  return (
    <div className="empty">
      <div className="ehalo">
        <div className="ering er1"></div>
        <div className="ering er2"></div>
        <div className="esat"></div>
        <div className="esat2"></div>
        <div className="ecore"></div>
      </div>
      <h1>No repos in orbit yet</h1>
      <p>Point Orrery at a directory where you keep your projects. It’ll discover every git repo inside and bring them into orbit.</p>
      <button className="pill" onClick={onAdd}><Icon name="plus" size={16} className="ico" />Add a directory</button>
    </div>
  );
}

function Scanning({ root }) {
  return (
    <div className="scanning">
      <div className="ehalo" style={{ width: 88, height: 88 }}>
        <div className="ering er1"></div>
        <div className="esat"></div>
        <div className="ecore"></div>
      </div>
      <div className="scanbar"><i></i></div>
      <div className="lbl">Scanning {root} → reading .git metadata…</div>
    </div>
  );
}

Object.assign(window, { TitleBar, Sidebar, RepoCard, StatusRow, EmptyState, Scanning });
