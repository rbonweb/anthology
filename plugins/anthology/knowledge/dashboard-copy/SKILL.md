---
name: dashboard-copy
description: "Words in data-heavy product UI: data labels, units, number and date formats, truncation, statuses, empty states and errors. Read when a screen shows data: dashboards, tables, charts, metrics, admin views."
---

# Dashboard copy

People read a dashboard to make a decision, so its words must help them read each number correctly. These rules cover the words around the data; general UI copy (buttons, forms, onboarding) is covered by ux-microcopy.

## When to use

- Any screen whose main job is showing data: dashboards, metric tiles, tables, charts, logs, admin and settings views with values.
- Writing a string table for UI someone else builds, or reviewing one.
- Writing the states a data view can be in: loading, empty, partial, stale and failed.

## Labels

- A label is a noun phrase in sentence case, 1 to 4 words. Bad: `Total Active Users Count`. Good: `Active users`.
- One metric has one name everywhere: tile, chart title, table header, filter and export. If the tile says `Active users`, the CSV column does not say `MAU`.
- Never show internal or column names. Bad: `rev_usd_7d`. Good: `Revenue (USD)`.
- When every value under a label shares a unit, put the unit once in the label (`Revenue (USD)`, `Latency (ms)`) and leave it off each value.
- An axis title says what is measured and in which unit (`Requests per second`). Leave it out when the chart title or label already says both.
- Legend order matches the order of the lines, bars or stack on screen. With 4 series or fewer, label each line directly at its end and drop the legend.
- A tooltip gives the full value, its unit and the date or period of that point: `Mar 12, 2026: 1,204,331 requests`.

## Units

- Put a space between a number and its unit: `12 MB`, `5 ms`, `30 s`. Write no space before `%`: `42%`.
- Place the currency symbol or code where the user's locale expects it, and let the formatter do it. When a view mixes currencies, use ISO codes (`USD 1,200`, `CAD 950`), because `$` alone can mean several currencies.
- Use one scale per column or axis. Bad: `980 KB`, `1.2 MB`, `0.9 GB` in one column. Good: all in `MB`, with the unit in the header.
- Pick one duration style for the whole product and keep it: `2 h 15 min`, not `2h15m` on one screen and `135 minutes` on the next.

## Numbers

- Right-align numbers in tables, and give every value in a column the same number of decimals, so digits line up and compare at a glance.
- Use the fewest decimals that still lead to the right decision. A conversion rate of `3.4%` needs one; a server count needs none.
- Abbreviate large numbers (`12.4K`, `3.1M`) only in metric tiles and chart axes, with one decimal at most, and show the full value in the tooltip. Never abbreviate in a table people compare or export.
- Format with the locale formatter, such as `Intl.NumberFormat`, never by joining strings. The same value is `1,234.50` in `en-US` and `1.234,50` in `de-DE`. `notation: "compact"` with `maximumFractionDigits: 1` gives `12.4K`.
- A change always carries a sign and says what it compares to. Bad: `4.2%`. Good: `+4.2% vs last 7 days`.
- Zero and missing are different facts. Show `0` when the value is zero. Show missing data with one mark the product defines (for example `—`), and give it a tooltip that says why: `No data: sensor offline`.
- Never mark a negative value by colour alone. Keep the minus sign; colour can add to it but never replace it.

## Dates and times

- Show dates and times in the user's time zone. Name the zone when the data spans zones or is shown in UTC: `09:00 UTC`. Storage stays in UTC, as the time rule in the ui-designer and implementer prompts says.
- Write the month as a word, in the order the user's locale expects: `7 November 2026` or `Nov 7, 2026`. Never `11/7/2026`, which reads as July in most of the world.
- Exports use ISO 8601 (`2026-11-07T09:00:00Z`), because they are read by software as well as people.
- Use relative times (`5 min ago`) only for the last 24 hours, and put the absolute date and time in the tooltip. After that, show the date.
- Ranges use "to", not a hyphen: `1 to 7 November 2026`. `Intl.DateTimeFormat` `formatRange` returns a dash, so build the "to" form from two formatted dates.
- Name date presets plainly: `Last 7 days`, `This month`, `Year to date`. Say whether today counts (`Last 7 days, including today`), because a partial day changes totals.
- Show how fresh the data is (`Updated 10:42`). When it is older than its refresh interval, say so next to the numbers: `Data is 3 hours old. Refresh`.

## Truncation

- Wrap labels and messages by default. Truncate only in fixed slots: table cells, tabs, chips and breadcrumbs.
- Cut names at the end with `…` (`Quarterly revenue for…`). Cut IDs, file paths, hashes and email addresses in the middle, because the start and end tell them apart: `a3f9…c21e`, `/var/log/…/error.log`.
- Never truncate numbers, units, dates, statuses, page titles or error messages. A shortened number can be read as a different number.
- Make the full text available on hover, on keyboard focus and to screen readers. A `title` attribute alone does not reach keyboard users.
- Set a maximum length for every string in the string table, and leave about 35% room for translated text to grow.

## Statuses

- Give each kind of object a small, fixed set of status words, such as `Running`, `Stopped`, `Failed` and `Needs review`. Each word means one state across the whole product; `Failed` on a job and `Failed` on a sync mean the same thing.
- Show status as icon, colour and text together. Colour alone fails people who cannot tell the colours apart, and it says nothing to a screen reader (WCAG 1.4.1).
- Sort and group by severity, worst first, so the item that needs action is the first one read.
- When a status needs action, put the time and the action next to it: `Failed 3 min ago · Retry`.

## Empty states

Each empty view gets one line saying what is missing and why, then one primary action. Tell these apart; one generic `No data` for all of them leaves people guessing.

| State | Example line | Action |
| --- | --- | --- |
| First use | `No reports yet. Reports appear here after your first export.` | `Create report` |
| No results | `No orders match these filters.` | `Clear filters` |
| Cleared | `All alerts resolved.` | none needed |
| No permission | `You need the Analyst role to see billing data.` | `Request access` |
| Failed to load | `Couldn't load orders. The server didn't respond.` | `Try again` |
| Loading | a skeleton or spinner, never an empty message | none |

- Never show an empty message before loading has finished.
- No blame and no jokes, least of all when something failed.
- A chart with no data keeps its axes and states the range it checked: `No data for 1 to 7 November`.
- A value of zero is not empty. Show `0`, not an empty state.

## Errors

- Say what happened and how to fix it, in plain words and as specifically as you can. Bad: `Invalid date`. Good: `End date must be after the start date`.
- Put any error code after the text, for support: `Couldn't export the report. Try again in a few minutes. (Error E4021)`.
- Do not write `Oops`, a bare `Something went wrong`, exclamation marks, or `invalid` as the whole message.
- Place each error where people look for it. Field errors sit inline next to the field. Errors for the whole page go in a summary or banner at the top. Toasts are only for brief events that block nothing.
- Keep everything the user entered, and say whether anything was saved: `Your filters are saved. The chart couldn't load.`
- A partial failure gives counts and a way to retry: `3 of 12 sources failed to load. Showing 9. Retry failed sources`.
- Timeouts, missing permission and usage limits each get their own message, because each has a different fix:
  - `The query took longer than 30 s and stopped. Narrow the date range and try again.`
  - `You don't have access to this workspace. Ask an admin to add you.`
  - `You've reached this month's limit of 50 exports. The limit resets on 1 December.`

## Check before you ship

- [ ] Every number has its unit and its period.
- [ ] Each metric has one name across tile, chart, table, filter and export.
- [ ] Zero shows `0`; missing data shows the product's mark with a reason.
- [ ] Dates use month words, and the time zone is named where it matters.
- [ ] Every truncated string has its full text on hover, focus and for screen readers.
- [ ] No status depends on colour alone.
- [ ] Every empty state says why it is empty and offers one action.
- [ ] Every error says what happened and how to fix it.
- [ ] The string table lists a key, the string, where it shows and a maximum length for each string.

## Sources

- Carbon Design System:
  - [Content](https://www.carbondesignsystem.com/building-blocks/foundations/content)
  - [Overflow content](https://www.carbondesignsystem.com/building-blocks/core/patterns/overflow-content)
  - [Empty states](https://www.carbondesignsystem.com/building-blocks/core/patterns/empty-states)
  - [Status indicators](https://www.carbondesignsystem.com/building-blocks/core/patterns/status-indicators)
  - [Data visualization](https://www.carbondesignsystem.com/building-blocks/data-visualization/overview)
- GOV.UK:
  - [Error message](https://design-system.service.gov.uk/components/error-message/)
  - [Dates](https://design-system.service.gov.uk/patterns/dates/)
  - [A to Z style guide](https://guidance.publishing.service.gov.uk/writing-to-gov-uk-standards/style-guides/a-to-z-style-guide/)
- Nielsen Norman Group, cited only:
  - [Error-message guidelines](https://www.nngroup.com/articles/error-message-guidelines/)
  - [Designing empty states in complex applications](https://www.nngroup.com/articles/empty-state-interface-design/)

[Contains public sector information licensed under the Open Government Licence v3.0.](https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/)

Principles adapted from the Carbon Design System (IBM, Apache License 2.0); the wording here is Anthology's.
