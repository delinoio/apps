---
"@servers/git-tracker-api": patch
---

Allow Actions OIDC pull request uploads to set an explicit baseline commit when the token's `repository_id` equals the connected repository's GitHub ID. The denial now applies only to tokens issued to another repository, such as a fork head, so fork tokens, uploads without a baseline and push uploads are unchanged. Add a mocked-GitHub unit test of the pull request branch and an Actions pull request upload to the PostgreSQL integration test.

<!-- public-changelog:start -->

Pull request jobs in the connected repository can now set an explicit `baseline` commit when uploading with GitHub Actions OIDC. Pull requests from forks still use the automatic baseline.

<!-- public-changelog:end -->
