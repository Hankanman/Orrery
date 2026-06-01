/* Orrery UI kit — Feed view (Phase 4: followed / starred browser).
   A GitHub-explore-style activity stream for the repos you follow. */
const { useState: useSF } = React;

const fmtStars = (n) => n >= 1000 ? (n / 1000).toFixed(n >= 10000 ? 0 : 1) + 'k' : n;
const FEED_ICON = { release: 'tag', star: 'star', push: 'git-commit-horizontal', newrepo: 'folder-git-2', issue: 'circle-alert' };
const FEED_VERB = {
  release: 'released', star: '', push: '', newrepo: 'created a repository', issue: 'opened an issue',
};

function Avatar({ initials, color }) {
  return <span className="avatar" style={{ color }}>{initials}</span>;
}

function FeedEvent({ e, onOpenRepo }) {
  const verb = e.type === 'push' ? `pushed ${e.count} commits to ${e.branch}` : FEED_VERB[e.type];
  return (
    <div className="fevent">
      <div className={'fev-badge ' + e.type}><Icon name={FEED_ICON[e.type]} size={18} /></div>
      <div className="fev-main">
        <div className="fev-top">
          <Avatar initials={e.initials} color={e.color} />
          <span className="actor">{e.actor}</span>
          {verb && <span className="verb">{verb}</span>}
          <span className="time">{e.time}</span>
        </div>

        {e.type === 'release' && (
          <React.Fragment>
            <div className="fev-headline">{e.version}<span className="tagpill">{e.tag}</span></div>
            <ul className="fev-notes">{e.notes.map((n, i) => <li key={i}>{n}</li>)}</ul>
          </React.Fragment>
        )}

        {e.type === 'star' && (
          <div className="fev-headline"><Icon name="star" size={16} style={{ color: 'var(--star)' }} /><span className="milestone">{e.text}</span></div>
        )}

        {e.type === 'push' && (
          <div className="fev-commits">
            {e.commits.map((c, i) => (
              <div className="fev-commit" key={i}>
                <Icon name="git-commit-horizontal" size={13} className="ico" />
                <span>{c.msg}</span><span className="sha">{c.meta}</span>
              </div>
            ))}
          </div>
        )}

        {e.type === 'newrepo' && (
          <React.Fragment>
            <div className="fev-headline">{e.repo.split('/')[1]}</div>
            <div className="fev-desc">{e.desc}</div>
          </React.Fragment>
        )}

        {e.type === 'issue' && (
          <div className="fev-headline" style={{ fontSize: 15 }}>#{e.number} · {e.title}</div>
        )}

        <div className="fev-foot">
          <span className="slug"><span className="ldot" style={{ background: e.color, color: e.color }}></span>{e.repo}</span>
          {e.stars != null && <span className="st"><Icon name="star" size={13} className="ico" />{fmtStars(e.stars)}</span>}
          <span className="open" onClick={() => onOpenRepo(e.repo)}>
            {e.type === 'release' ? 'View release' : e.type === 'issue' ? 'View issue' : 'View'}
            <Icon name="chevron-right" size={13} className="ico" />
          </span>
        </div>
      </div>
    </div>
  );
}

function FeedView({ feed, following, filter, onOpenRepo }) {
  const [follows, setFollows] = useSF(() => Object.fromEntries(following.map(f => [f.id, true])));
  const list = filter === 'all' ? feed
    : filter === 'activity' ? feed.filter(e => e.type === 'push' || e.type === 'newrepo' || e.type === 'issue')
    : feed.filter(e => e.type === filter);

  // group by day, preserving order
  const days = [];
  list.forEach(e => {
    let g = days.find(d => d.day === e.day);
    if (!g) { g = { day: e.day, items: [] }; days.push(g); }
    g.items.push(e);
  });

  // trending = unique repos by stars
  const seen = {};
  const trending = [...feed].filter(e => e.stars != null)
    .filter(e => (seen[e.repo] ? false : (seen[e.repo] = true)))
    .sort((a, b) => b.stars - a.stars).slice(0, 5);

  return (
    <div className="feedwrap">
      <div className="feed">
        {days.map(g => (
          <React.Fragment key={g.day}>
            <div className="fev-day">{g.day}</div>
            {g.items.map(e => <FeedEvent key={e.id} e={e} onOpenRepo={onOpenRepo} />)}
          </React.Fragment>
        ))}
        {list.length === 0 && <div className="fev-day">Nothing here yet</div>}
      </div>

      <div className="feed-rail">
        <div>
          <div className="rail-h">Following · {following.length}</div>
          {following.map(f => (
            <div className="follow-row" key={f.id}>
              <Avatar initials={f.initials} color={f.color} />
              <span>{f.name}</span>
              <span className={'fbtn' + (follows[f.id] ? ' on' : '')}
                onClick={() => setFollows(s => ({ ...s, [f.id]: !s[f.id] }))}>
                {follows[f.id] ? 'Following' : 'Follow'}
              </span>
            </div>
          ))}
        </div>
        <div>
          <div className="rail-h">Trending in your orbit</div>
          {trending.map(t => (
            <div className="trend-row" key={t.repo} onClick={() => onOpenRepo(t.repo)}>
              <span className="ldot" style={{ background: t.color, color: t.color }}></span>
              <span className="nm">{t.repo}</span>
              <span className="st"><Icon name="star" size={12} className="ico" />{fmtStars(t.stars)}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

Object.assign(window, { FeedView, FeedEvent });
