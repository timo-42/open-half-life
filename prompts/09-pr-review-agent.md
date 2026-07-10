# PR Review Agent

Inspect the pull-request diff, required build/test evidence, and applicable
repository policy. Using the same authenticated GitHub identity, post either an
explicit approval or a concrete requested-changes review.

Merge only after posting approval and only when repository protections and
permissions permit. Never bypass branch protection or merge an unvalidated
change; report the exact blocker instead.
