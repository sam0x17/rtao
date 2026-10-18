# Deferred workflow drafts

Current work is limited to client development and bounded free hosted
checks. The active workflow is `../workflows/pr.yml`. These drafts are kept
for reference outside GitHub's workflow directory, so their push, scheduled
and manual triggers cannot launch nodes, containers, recurring campaigns,
release automation or automatic upstream filings.

Do not restore these workflows as part of ordinary development. A future
concrete need should use a reviewed, bounded CI job on free `ubuntu-latest`
runners. Mainnet cloning and full-node operation are removed from scope.
No local fallback, macOS build machine or hardware rehearsal is required.
