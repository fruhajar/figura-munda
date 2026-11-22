# Rulesets

A ruleset lives in GitHub's API rather than in the repository, so this directory
holds the payload that produced the one in force. Committing it means the
protection can be read, reviewed and restored like anything else here, and that
a change to it leaves a trace.

## What `main.json` does

| Rule | Why |
|---|---|
| No deletion | `main` is the published history. |
| No force push | A rewritten public history breaks every clone and every quoted commit. |
| Four checks must pass | The suite, the fixture contract, the trust posture and the sign-off. A commit reaching `main` has to have been through all four. |

The checks are named exactly as the jobs in
[`../workflows/checks.yml`](../workflows/checks.yml). Renaming a job without
renaming it here would leave a check required that can never report, which
blocks every push to `main`, so the two files move together.

Each check is named and nothing else: no `integration_id` pinning it to the
GitHub Actions app. Pinning is tidier, and a wrong id is a required check that
can never report, which blocks `main` until somebody notices. With no other app
installed there is nothing else that could answer to these names, so the id can
be added later from what the UI shows.

`strict_required_status_checks_policy` is off, so a fast-forward of commits that
already passed on `dev` is allowed without re-running anything. That is the
normal way work lands here: push to `dev`, let the four jobs run, then

```sh
git checkout main && git merge --ff-only dev && git push origin main
```

`bypass_actors` is empty, which includes the repository owner. The protection is
real rather than advisory, and the cost is that a broken runner or a GitHub
outage means waiting rather than forcing. To grant the owner an escape hatch,
add `{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}`
and say so in the commit that does it.

## Apply it

```sh
gh api --method POST repos/:owner/:repo/rulesets --input .github/rulesets/main.json
```

## Change it

Edit the file, then update in place rather than creating a second ruleset that
overlaps the first:

```sh
gh api repos/:owner/:repo/rulesets --jq '.[] | "\(.id)\t\(.name)"'
gh api --method PUT repos/:owner/:repo/rulesets/RULESET_ID --input .github/rulesets/main.json
```

## Check what is in force

```sh
gh api repos/:owner/:repo/rulesets --jq '.[].name'
gh api repos/:owner/:repo/rules/branches/main --jq '.[].type'
```

The second command answers the only question that matters: what actually
applies to `main` right now, whatever this file says.

## Not included, deliberately

- **Required pull requests.** A solo maintainer reviewing their own pull request
  is a formality, and the checks already gate the push. Worth adding the day a
  second person has write access.
- **Required signed commits.** Commits here are signed off under the DCO, which
  is a statement about provenance, not GPG-signed. Turning this on without
  setting up a key first locks the repository against its own owner.
- **Tag protection.** There are no releases yet. When there are, a second
  ruleset targeting `refs/tags/v*` with `deletion` and `non_fast_forward` stops
  a published tag being moved.
