---
"@apps/lane": patch
---

Give the Events page a load owner (`EventsLoader`) so that only the latest load for the
current adapter and filters can publish receivers, events, continuation cursor and load
status. Continuation cursors are tied to the query that produced them, obsolete receiver
page chains stop, and unmounted loads cannot publish.

<!-- public-changelog:start -->

Events now always show results for the repository and state filters you selected. Slow
responses from an earlier filter no longer replace the list, and Load more events no
longer continues from a previous filter.

<!-- public-changelog:end -->
