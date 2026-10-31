# Inbox, Feed & Explore

Beyond your local repos, Orrery pulls in what's happening across your git hosts and splits it into three purpose-built views.

## Inbox

The Inbox is for things that need *you*: open pull requests, review requests, assigned issues, and notifications — grouped and counted.

![Inbox — PRs, reviews, issues, notifications](/shots/inbox.png)

It answers "what's waiting on me right now?" across your hosts:

| Host | Your PRs | Review requests | Assigned issues | Notifications |
|---|---|---|---|---|
| **GitHub** | PRs you opened | PRs awaiting your review | Issues assigned to you | Unread notifications |
| **GitLab** (gitlab.com and trusted self-hosted) | MRs you opened or are assigned | MRs where you're a reviewer | Issues assigned to you | Pending to-do items |
| **Gitea / Forgejo / Codeberg** | PRs you opened | PRs awaiting your review | Issues assigned to you | Unread notifications |

Each connected instance is queried separately, so one host being down or rate-limited never hides another's items. Orrery also polls your hosts in the background and raises desktop notifications for new PRs, review requests, and CI failures — see [Notifications & tray](./notifications).

Every inbox item also records how long it has been waiting: how long a PR you opened has been open, and how long a review request has been waiting on you (on GitLab, from the moment the review was requested; on GitHub and Gitea, from when the PR was opened). Items past a threshold count as **stale**: by default 7 days for your own PRs and 2 days for review requests (`stalePrDays` / `staleReviewDays` in the [configuration](./configuration), `0` to turn one off). Drafts are never stale. *(Planned: stale badges in the Inbox. The ages are already tracked; the badges come in a later release.)*

::: info Multi-host Inbox, coming to the view
The GitLab and Gitea sources are implemented and already feed background notifications and the attention model. The Inbox, Feed and Explore views themselves switch over to them in a following release; until then they show GitHub.
:::

## Feed

The Feed is the social/news stream — releases and activity from the repos and people you follow or have starred (think a personal "release radar").

![Feed — releases and activity](/shots/feed.png)

Releases show their tag; starred/created/forked events show who did what. On GitLab, the feed carries the latest release of each project you've starred (your 30 most recently active stars). Gitea has no feed source yet. The feed is cached for 30 minutes and refreshes on demand.

## Explore

Explore is a browser for the repos you've starred, with one-click clone straight into a workspace root.

![Explore — starred repos](/shots/explore.png)

Each card shows the description, language, and star count; **Clone** drops it into your first configured root so it shows up in Mission Control on the next scan.

## Caching & offline

Host data (enrichment, feed, notifications) is cached locally with sensible TTLs and falls back to the last good snapshot when offline — so these views stay useful without a connection, and visibility/stars survive restarts without a re-fetch.
