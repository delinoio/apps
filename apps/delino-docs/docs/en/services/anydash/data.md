# Data, retention and sharing

This page explains what AnyDash uploads, how long it keeps data, how deletion works, what team members can see, and what estimated cost means.

## What AnyDash uploads

The collector on your device does the work locally and uploads only results.

AnyDash uploads:

- **Local usage aggregates** for each device, tool, model and 15-minute period: session count, input, output, cache-read and cache-write token counts, and the model name.
- **External metric results** that the collector reads from your providers, for example used percentages, reset times, project status, usage quantities, plans and costs.
- **Device and collector information:** the device name, platform, collector version, parser state and the number of skipped records.

AnyDash never uploads:

- API tokens, CLI sign-ins or other provider credentials. They stay on the device.
- Prompts, responses or session contents.
- Individual session records, file paths, session identifiers or project information from your session files.

AnyDash does not collect product analytics.

## Retention

| Data                                                  | Kept for                                                            |
| ----------------------------------------------------- | ------------------------------------------------------------------- |
| Local usage in 15-minute periods                      | 90 days                                                             |
| History of external metrics (one value for each hour) | 90 days                                                             |
| Latest value of each external metric                  | Until a newer value replaces it, or until you delete the connection |
| Activity log                                          | 30 days                                                             |
| Usage that waits for upload on an offline device      | Up to 90 days on the device                                         |
| Collector logs on the device                          | 7 days or 50 MiB, whichever limit comes first                       |

Hourly and daily views are calculated in the time zone that you select, including time zones with 30- or 45-minute offsets and daylight saving time changes. The collector can correct usage from the last 90 days, for example when a session file grows; the correction replaces the earlier value and never counts usage twice. AnyDash cannot recover data older than 90 days, also after an offline period.

The activity log in **Settings**, then **Activity** shows events for your devices, connections and account from the last 30 days. Team Owners and Admins also see the dashboard and sharing events of their team.

## Deletion

When you delete a connection, a device or a dashboard:

1. Access stops immediately.
2. AnyDash deletes the data from its primary database within 24 hours.
3. Backup copies expire with the backup retention period, which is at most 30 days.

What each deletion removes:

- **Connection:** collection stops, the registered devices delete their stored credential for it, and its stored values, history and shared widget data are deleted. Widgets that used only this connection show "Sharing ended" to team members.
- **Device:** its credentials are revoked, collection and upload stop, and its local usage data is deleted. The history of external connections stays, because it belongs to the connection. The device is no longer a collection device for any connection. If it was the last device registered for a connection, that connection stops refreshing.
- **Dashboard:** the dashboard and its widgets are removed for everyone in the space. Your connections and usage data stay.

AnyDash never deletes the Codex or Claude Code session files on your devices.

### Delete all AnyDash data

In **Settings**, select **Delete AnyDash data** to delete all of your AnyDash data: devices, connections, usage history, personal dashboards and the widgets you shared with teams. Access stops immediately, and deletion completes within 24 hours. Your Delino account stays. To delete the account itself, follow the account deletion link in the same dialog. When you delete your Delino account, AnyDash deletes all of your AnyDash data with the same deadlines.

## Team sharing

A team space uses the roles of your Delino team:

| Role                  | Team dashboards               |
| --------------------- | ----------------------------- |
| Owner, Admin          | Create, delete, edit and view |
| Member                | Edit and view                 |
| Billing only, no role | No access                     |

When you add a widget with your own data to a team dashboard, AnyDash asks you to confirm "Share this widget with {team}?". Team members can then see only the data that this widget displays. They cannot see your connection, your credentials, your devices, other metrics, other periods or any other data.

On a widget that another person shared, team members can change only its position, size, title and presentation. Only the person who owns the data can change its period, filters and sharing. A team role does not override this rule.

### End sharing

Sharing ends when the data owner selects **Stop sharing**, leaves the team or loses their team role, deletes the source connection or device, or deletes their AnyDash data. Access on the server ends immediately. Apps and native widgets remove the cached data of that widget at their next launch or refresh and show "Sharing ended". A device that is offline can show the old data until its next launch or refresh. AnyDash cannot remove data that someone already saw or saved separately. Rejoining the team does not restore an ended share.

### Team state

If a team is blocked or is being deleted, you cannot view or edit its dashboards. The space shows "This space is unavailable". When a team is deleted, its team dashboards are deleted.

## Estimated cost

AnyDash shows an estimated cost for Codex and Claude Code local usage, labeled "Est. cost (USD, estimated)".

- It is calculated from the token counts of each model and public API prices per model, including cache-read and cache-write prices. AnyDash uses the community-maintained LiteLLM price list and checks it daily for changes.
- It is in USD. AnyDash does not convert currencies.
- It is **not your bill and not your subscription fee**. It shows what the same tokens would cost at public API prices. If you use a subscription plan, your actual charges are different.
- Each period uses the prices that were in effect at that time. AnyDash never recalculates past values when prices change.
- If a model has no public price, or a required token breakdown is missing, the cost shows "Can't be calculated". The token counts are still shown.

Costs from Vercel and Cloudflare are different: they come from the provider's billing data, in the provider's currency, and are labeled as billed cost. Supabase cost is "Not provided".

## Cross-device duplication

AnyDash adds the usage of all of your devices together. It does not check whether two devices read the same sessions. If you copy or sync the same session files to more than one device, for example with a file sync service or a shared home folder, their usage is counted once for each device. When a widget combines data from two or more devices, it shows "Usage from {n} devices is combined." To avoid double counting, disable the copied folder on all devices except one with [`anydash paths disable`](/services/anydash/cli#paths).

## Quotas and limits

| Limit                      | Value             |
| -------------------------- | ----------------- |
| Collector devices          | 20 per user       |
| External connections       | 50 per user       |
| Dashboards                 | 50 per space      |
| Widgets                    | 100 per dashboard |
| Requests from your sign-in | 600 per minute    |
| Requests from a device     | 60 per minute     |
| Upload size                | 1 MiB per request |

A request over a quota shows "Limit reached". A request over a rate limit is rejected with the time when you can try again. See [Troubleshooting](/services/anydash/troubleshooting).
