# Compliance, and the policy layer

What this tool is in legal terms, what it leaves to the operator, and what an
organisational policy layer would need before it could exist. Summarised in
[`../README.md`](../README.md); the detail is here because a buyer's first three
questions are all in this file.

## What this tool is, legally

It is a data-minimisation tool that runs entirely on the user's machine. It is
not a processor of anyone's data: nothing leaves the device, there is no
operator, and there is nothing to make a data-processing agreement about. For a
business buyer the relevant framing is GDPR Article 5(1)(c) and Article 25, where
removing what you do not need before a file leaves the building is exactly the
measure those articles ask for. That is the commercial hook, and it is a true
one.

## What the operator is responsible for

**Metadata is part of a document for evidentiary purposes in most
jurisdictions.** A file under a litigation hold or a disclosure obligation should
not be cleaned, and removing its metadata can be spoliation whatever the intent
was. This is also why `normalise` exists and `falsify` does not: a fixed constant
makes no false claim, while a plausible fake timestamp is a statement that is not
true.

Knowing that is the point of writing it down here. Acting on it is the operator's
job, not the tool's, and the control is the obvious one: do not put such a file
in a folder that is being cleaned, and do not point the tool at it. A document
under hold belongs in a document management system that knows it is under hold.
This tool cannot know, because there is nothing in a file that says so, and a
guard that depended on the user marking the file would be the same act as the
user not putting it there.

So there is no legal-hold feature and there is not going to be one. What the tool
does instead is refuse to be surprising: `inspect` examines without touching
anything, the report names every field before a byte is written, the desktop
window will not clean until the first phase has run, and anything that removes
content a reader can see is listed in full and waits for a tick.

The one place to be deliberate is the mobile runner, because it is the only part
that cleans without anyone present: it sweeps the folders it was given, in place,
on a schedule, including files that arrived long after those folders were chosen.
That is what it is for, and it is why `DCIM/Camera` is the example and
`Documents` is not.

**Cleaning is not redaction.** `enforce` removes text that a filled rectangle
only appeared to hide, which is a genuine fix for a genuine failure, but a tool
that reflows or re-encodes is not a substitute for a redaction workflow with
review and sign-off. Nothing here should be described as one.

## The policy layer, and the problem in it

Profiles are already a policy format in everything but name: a file, validated
on load, carried between a desktop and a phone. What would make them an
organisational policy is three things, none of which exist:

1. **Locked cells.** A profile cannot currently say "Location is removed and the
   user may not change that". Every cell is advisory, so a distributed profile is
   a suggestion.
2. **Provenance.** Nothing signs a profile, so nothing can tell an
   administrator's profile from a file the user edited. Locked cells without
   signing would be security theatre.
3. **Precedence.** With more than one profile in play there is no rule for which
   wins, and the safe answer (the strictest cell from any applicable profile)
   is not obviously the one an administrator expects.

**And the audit trail has a problem worth stating before anyone builds it.** An
audit trail for this tool is a list of which files a person had, when, and what
was in them. That is a more sensitive artefact than most of what it would be
auditing. The mobile journal already runs into this: it is excluded from cloud
backup and device transfer for exactly that reason. A compliance feature that
produces a durable, centralised, exportable record of everyone's documents would
undo the thing the product is for, so if it is built it should record **decisions
and counts, not filenames and field values**, and that constraint should be in
the design from the start rather than retrofitted after a customer asks for
names.
