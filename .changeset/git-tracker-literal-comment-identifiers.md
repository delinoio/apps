---
"@servers/git-tracker-api": patch
---

Render uploaded text in the pull request comment literally. Dataset, row and column identifiers, values, units and changes now appear only inside inline code spans (backtick fence one longer than the longest inner run, space padding for edge backticks, line breaks replaced by spaces, pipes escaped), and each is truncated to 120 characters plus an ellipsis, so uploaded Markdown, links, images, mentions and table syntax can no longer change the comment. Every table row keeps four cells; the marker, headings, status text, execution link and 60,000 byte limit are unchanged. The execution details page keeps full values.

<!-- public-changelog:start -->

Pull request comments now show dataset, item and column names exactly as uploaded, in code formatting, and shorten names longer than 120 characters. Names can no longer inject links, images, mentions or table breaks into the comment.

<!-- public-changelog:end -->
