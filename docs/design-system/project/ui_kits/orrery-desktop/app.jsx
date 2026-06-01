/* Orrery UI kit — App. Wires the full flow: onboarding → scan → grid,
   with command palette, detail drawer, settings, filters, sort, list/grid. */
const { useState: useS, useEffect: useE } = React;

function App() {
  const DATA = window.ORR_DATA;
  const [stage, setStage] = useS('empty');      // empty | scanning | grid
  const [view, setView] = useS('grid');          // grid | list (card layout)
  const [mode, setMode] = useS('grid');          // grid | feed (which screen)
  const [feedFilter, setFeedFilter] = useS('all');
  const [activeRoot, setActiveRoot] = useS('all');
  const [langFilter, setLangFilter] = useS(null);
  const [statusFilter, setStatusFilter] = useS(null); // dirty | ahead | starred | stale
  const [sort, setSort] = useS('activity');      // activity | name | stars
  const [favs, setFavs] = useS(() => Object.fromEntries(DATA.repos.map(r => [r.slug, r.fav])));
  const [palette, setPalette] = useS(false);
  const [detail, setDetail] = useS(null);
  const [settings, setSettings] = useS(false);
  const [toast, setToast] = useS(null);

  useE(() => {
    const h = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); setPalette(true); }
      else if ((e.metaKey || e.ctrlKey) && e.key === ',') { e.preventDefault(); setSettings(true); }
    };
    window.addEventListener('keydown', h);
    return () => window.removeEventListener('keydown', h);
  }, []);

  const startScan = () => { setStage('scanning'); setTimeout(() => setStage('grid'), 1700); };

  const showToast = (msg, color) => { setToast({ msg, color }); setTimeout(() => setToast(null), 2200); };
  const onIDE = (r) => { showToast(`code ${r.path}`, 'var(--accent)'); setDetail(null); };
  const onAgent = (r) => { showToast(`◗ agent → ${r.name}`, 'var(--ai)'); setDetail(null); };
  const onFav = (r) => setFavs(f => ({ ...f, [r.slug]: !f[r.slug] }));
  const onOpenRepo = (slug) => {
    const r = DATA.repos.find(x => x.slug === slug);
    if (r) setDetail(r); else showToast(`Opening ${slug} on host`, 'var(--accent)');
  };

  let repos = DATA.repos.map(r => ({ ...r, fav: favs[r.slug] }));
  if (activeRoot !== 'all') repos = repos.filter(r => r.root === activeRoot);
  if (langFilter) repos = repos.filter(r => r.lang === langFilter);
  if (statusFilter === 'dirty') repos = repos.filter(r => r.dirty > 0);
  if (statusFilter === 'ahead') repos = repos.filter(r => r.ahead > 0);
  if (statusFilter === 'starred') repos = repos.filter(r => r.fav);
  if (statusFilter === 'stale') repos = repos.filter(r => r.stale);
  const order = { activity: (a, b) => 0, name: (a, b) => a.name.localeCompare(b.name), stars: (a, b) => b.stars - a.stars };
  repos = [...repos].sort(order[sort]);

  const sortLabel = { activity: 'Recent activity', name: 'Name', stars: 'Stars' }[sort];
  const cycleSort = () => setSort(s => s === 'activity' ? 'name' : s === 'name' ? 'stars' : 'activity');

  const chips = [
    { id: 'dirty', label: 'Dirty', icon: 'circle-dot' },
    { id: 'ahead', label: 'Ahead', icon: 'arrow-up' },
    { id: 'starred', label: 'Starred', icon: 'star' },
    { id: 'stale', label: 'Stale', icon: 'clock' },
  ];

  return (
    <div className="app">
      <div className="starfield"></div>
      <TitleBar roots={DATA.roots} view={settings ? 'settings' : view}
        onSearch={() => setPalette(true)} onSettings={() => setSettings(true)}
        onRefresh={() => { setStage('scanning'); setTimeout(() => setStage('grid'), 1200); }} />

      <div className="body">
        {stage === 'grid' && (
          <Sidebar roots={DATA.roots} repos={DATA.repos} activeRoot={activeRoot} setActiveRoot={setActiveRoot}
            langFilter={langFilter} setLangFilter={setLangFilter} mode={mode} setMode={setMode} />
        )}

        <div className="gridwrap">
          {stage === 'empty' && <EmptyState onAdd={startScan} />}
          {stage === 'scanning' && <Scanning root="~/dev" />}
          {stage === 'grid' && mode === 'feed' && (
            <React.Fragment>
              <div className="toolbar">
                <span className="tb-title">Feed</span>
                <span className="tb-sub">· following {DATA.following.length}</span>
              </div>
              <div className="chiprow">
                {[{ id: 'all', label: 'All', icon: 'sparkles' }, { id: 'release', label: 'Releases', icon: 'tag' },
                  { id: 'activity', label: 'Activity', icon: 'git-commit-horizontal' }, { id: 'star', label: 'Stars', icon: 'star' }].map(c => (
                  <div key={c.id} className={'fchip' + (feedFilter === c.id ? ' on' : '')} onClick={() => setFeedFilter(c.id)}>
                    <Icon name={c.icon} size={13} className="ico" />{c.label}
                  </div>
                ))}
              </div>
              <FeedView feed={DATA.feed} following={DATA.following} filter={feedFilter} onOpenRepo={onOpenRepo} />
            </React.Fragment>
          )}
          {stage === 'grid' && mode === 'grid' && (
            <React.Fragment>
              <div className="toolbar">
                <span className="tb-title">{activeRoot === 'all' ? 'All repos' : DATA.roots.find(r => r.id === activeRoot).path}</span>
                <span className="tb-sub">· {repos.length}</span>
                <div className="tb-spacer" style={{ flex: 1 }}></div>
                <div className="sortpill" onClick={cycleSort}><Icon name="arrow-down-up" size={14} />{sortLabel}</div>
                <div className="seg">
                  <button className={view === 'grid' ? 'on' : ''} onClick={() => setView('grid')}><Icon name="layout-grid" size={16} /></button>
                  <button className={view === 'list' ? 'on' : ''} onClick={() => setView('list')}><Icon name="list" size={16} /></button>
                </div>
              </div>
              <div className="chiprow">
                {chips.map(c => (
                  <div key={c.id} className={'fchip' + (statusFilter === c.id ? ' on' : '')}
                    onClick={() => setStatusFilter(statusFilter === c.id ? null : c.id)}>
                    <Icon name={c.icon} size={13} className="ico" />{c.label}
                  </div>
                ))}
                {(langFilter || statusFilter || activeRoot !== 'all') && (
                  <div className="fchip" onClick={() => { setLangFilter(null); setStatusFilter(null); setActiveRoot('all'); }}>
                    <Icon name="x" size={13} className="ico" />Clear
                  </div>
                )}
              </div>
              <div className={'grid ' + view}>
                {repos.map(r => (
                  <RepoCard key={r.slug} r={r} view={view} onOpen={setDetail} onFav={onFav} onIDE={onIDE} onAgent={onAgent} />
                ))}
              </div>
            </React.Fragment>
          )}
        </div>
      </div>

      {palette && <CommandPalette repos={DATA.repos} onClose={() => setPalette(false)}
        onPick={(r) => { setPalette(false); setDetail(r); }}
        onCommand={(id) => {
          setPalette(false);
          if (id === 'settings') setSettings(true);
          else if (id === 'feed') setMode('feed');
          else if (id === 'grid') setMode('grid');
          else if (id === 'list') { setMode('grid'); setView(v => v === 'grid' ? 'list' : 'grid'); }
          else if (id === 'rescan' || id === 'add') { setStage('scanning'); setTimeout(() => setStage('grid'), 1200); }
        }} />}
      {detail && <RepoDetail r={{ ...detail, fav: favs[detail.slug] }} onClose={() => setDetail(null)} onIDE={onIDE} onAgent={onAgent} />}
      {settings && <Settings onClose={() => setSettings(false)} />}

      {toast && (
        <div style={{
          position: 'fixed', bottom: 24, left: '50%', transform: 'translateX(-50%)', zIndex: 90,
          display: 'flex', alignItems: 'center', gap: 9, padding: '11px 16px',
          background: 'rgba(18,24,38,.92)', backdropFilter: 'blur(20px)',
          border: '1px solid var(--border-strong)', borderRadius: 'var(--r-sm)',
          font: 'var(--text-data)', color: toast.color, boxShadow: 'var(--shadow-pop)',
          animation: 'popin .2s var(--ease-spring)',
        }}>
          <Icon name="terminal" size={15} /> {toast.msg}
        </div>
      )}
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<App />);
