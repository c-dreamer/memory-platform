# Evidence-Grounded Structural Retrieval

## Production Baseline

`content_nodes` is a derived local hierarchy. It never replaces, compacts, or
deletes source records. The production adapters cover:

- document root;
- heading and subsection leaves;
- stable source-local keys, byte ranges, checksums, section paths, and vault scope;
- the source record's storage tier.

Markdown documents receive native document and heading/subsection nodes.
Memories and sessions receive one atomic fact or summary node, respectively.
This gives every current first-party ingestion path deterministic provenance
while leaving richer source-specific trees (mail messages, PDFs, tool turns,
and repository symbols) behind their own evaluation gates.

Only active nodes are eligible for the Neon projection. Node body text is
always replaced with an empty placeholder before it is sent to Neon; checksums,
scope, hierarchy, and other safe metadata remain available for parity/audit.
Archive-tier raw text is not made eligible by this index. A node rebuild deletes
and recreates only nodes belonging to one source record in one transaction.

`memory_search` remains keyword-capable when embeddings are unavailable. It
first performs the existing source retrieval, then obtains structural evidence
only for already-selected documents. Structural evidence can boost and explain
a result, but cannot introduce a new source or bypass existing retrieval
filters. Supplying `scope` makes document retrieval fail closed: an unindexed
or other-scope document is omitted rather than returned across a vault boundary.

Vault import is also scope-safe. When an import path is a parent containing two
or more known vault roots, `vault-sync` and `ingest vault` refuse to proceed
without `--scope`/`VAULT_SCOPE`. Historical records without a verified scope
remain `unclassified`; do not bulk-label them from folder names. Re-ingest one
real vault root at a time, with its explicit scope, before enabling scoped
retrieval for that vault.

## Operations

Use `structure-index` to backfill existing documents. It is read-only unless
`--apply` is supplied. Start with a small `--limit` canary and an explicit
`--scope`. An explicit document scope is also recorded in existing document
frontmatter during an apply run, allowing the pre-ranking scope filter to work
for older imports. The command does not upload to Neon, call an embedding
provider, touch archive bundles, or alter source text or paths.

After a successful local canary, the normal durable outbox carries active node
metadata to Neon. Do not use a full dump or target reset for this feature.

`structure-eval --confirm-test-database` seeds a small checked-in fixture,
verifies selected leaves and scopes, then deletes its own records. It must run
only on a disposable local database or isolated Neon branch.

## Release Gates

The following features are not production-enabled by this migration:

- automatic LLM summaries;
- cross-encoder reranking;
- MLX/LoRA STAIR navigator training;
- any cross-vault expansion.

They may be enabled only after a versioned evaluation set demonstrates no scope
leaks, complete source provenance, stable temporal behavior, and a measured
quality improvement. A navigator can select only current valid node IDs and
must fall back to deterministic retrieval on invalid, stale, unavailable, or
out-of-scope output. Summaries are retrieval aids, not authoritative facts, and
must retain child checksum links.
