# Inbox, Feed & Explore

Beyond your local repos, Orrery pulls in what's happening across your git hosts and splits it into three purpose-built views.

## Inbox

The Inbox is for things that need *you*: open pull requests, review requests, assigned issues, and notifications — grouped and counted.

![Inbox — PRs, reviews, issues, notifications](/shots/inbox.png)

It answers "what's waiting on me right now?" across GitHub and GitLab. Orrery also polls these in the background and raises desktop notifications for new PRs, review requests, and CI failures — see [Notifications & tray](./notifications).

## Feed

The Feed is the social/news stream — releases and activity from the repos and people you follow or have starred (think a personal "release radar").

![Feed — releases and activity](/shots/feed.png)

Releases show their tag; starred/created/forked events show who did what. The feed is cached for 30 minutes and refreshes on demand.

## Explore

Explore is a browser for the repos you've starred, with one-click clone straight into a workspace root.

![Explore — starred repos](/shots/explore.png)

Each card shows the description, language, and star count; **Clone** drops it into your first configured root so it shows up in Mission Control on the next scan.

## Caching & offline

Host data (enrichment, feed, notifications) is cached locally with sensible TTLs and falls back to the last good snapshot when offline — so these views stay useful without a connection, and visibility/stars survive restarts without a re-fetch.
