# Releasing muse-acp

The only destination is `BrokkAi/muse-acp` GitHub Releases. Cargo explicitly
sets `publish = false`; there are no registry packages, containers, update feeds,
documentation deployments, signing or notarization services. There is one Rust
binary with no runtime dependencies and no monorepo package publication order.

Each `v<version>` release has eleven assets: `install.sh`, plus an archive and
`.sha256` sidecar for each of:

- `x86_64-unknown-linux-gnu` (tar.gz)
- `aarch64-unknown-linux-gnu` (tar.gz)
- `x86_64-apple-darwin` (tar.gz)
- `aarch64-apple-darwin` (tar.gz)
- `x86_64-pc-windows-msvc` (zip)

Names are `muse-acp-v<version>-<target>.<format>`. Each archive includes the
binary, README, LICENSE, NOTICE, and `release.json` with the exact commit,
version, platform, file hashes and permissions. Windows archives have flat
paths; Unix archives retain the installer's versioned parent directory.

## Preparation (does not publish)

1. Fetch master and merge it into the job's `brb/release-*` topic branch.
   Preserve unreleased local work. Increment Cargo.toml and Cargo.lock together
   and update CHANGELOG.md. Never reuse a completed release version.
2. Run the contributing checks and `python3 -m unittest discover -s scripts -p
   'test_*.py'`. Push the topic branch and open a PR to master. `ci.yml` runs on
   pushes and PRs. `release.yml` runs on master and release-topic pushes; these
   runs build/test all five platforms and validate the publisher without
   publishing. A manual dispatch also only validates, and additionally compares its rebuilt
   payloads with a prior successful push preflight at the same commit.
3. The `publisher` job uses the repository-scoped ephemeral `github.token`, with
   `contents: write` and `actions: read`, and no environment or external secret. It creates,
   updates, and deletes a disposable private draft (no assets or tag), proving
   that the actual job token can manage releases. The evidence survives draft
   deletion. Ref lookups assert no probe tag was created. Organizations must
   permit Actions, these runners/actions, and the job's write token. A denied
   token or approval is a blocking error; local gh credentials are not proof.
4. Merge the PR normally, respecting approvals and checks. Fetch master and
   detach this workspace at the actual merged commit. Wait for both `ci` and
   `release` push runs at that exact SHA to succeed. Dispatch `release.yml` against a
   branch still pointing at that exact commit and require its successful
   conclusion and matching head SHA. This non-publishing rebuild checks payload
   reproducibility and exercises Actions artifact reads in the publisher job.
   The authorization check requires this dispatch evidence. Run:

   ```sh
   RELEASE_COMMIT=$(git rev-parse HEAD) RELEASE_TAG=v0.4.5 python3 scripts/release.py build
   RELEASE_COMMIT=$(git rev-parse HEAD) RELEASE_TAG=v0.4.5 python3 scripts/release.py authorization
   RELEASE_COMMIT=$(git rev-parse HEAD) RELEASE_TAG=v0.4.5 python3 scripts/release.py version
   ```

   Set the tag to the proposed version. These commands are non-publishing:
   they require successful exact-SHA CI and all release jobs, inspect publisher
   steps and unexpired Actions artifacts, and validate all packaged metadata.
   Version additionally checks the tag/release namespace and any existing
   assets. Authentication, network errors, missing/expired evidence, skipped
   jobs, and conflicting versions fail closed. Artifact retention is 30 days;
   rerun the unchanged commit's non-publishing workflow if evidence expires.

## Publication (separate authorization/phase)

Only an explicit push of a matching `v*` tag publishes. Push the annotated tag
from an authorized CLI identity; do not assume tags pushed with GITHUB_TOKEN
will trigger Actions. Never create/push tags during preflight. The publication
workflow builds all platforms before its publisher starts. Release builds pin
Rust 1.98.1. Windows uses the MSVC `/Brepro` linker option to avoid changing
PE timestamps and identifiers (see [LLVM's reproducible-linking notes](https://blog.llvm.org/2019/11/deterministic-builds-with-clang-and-lld.html)).
Before uploading, the tag run must compare every unpacked payload with the
successful branch preflight artifacts at the same commit. A runner/toolchain
change producing different binaries is a blocking error, never an upload
followed by a failed verification. The publisher's read-only Actions permission
is used to retrieve that exact-commit evidence. Before the first
release asset upload, it validates all artifacts, exact version/tag/commit,
existing release contents, and its own fresh publishing credential. It creates
missing draft state, uploads only missing assets, reads each new upload back
and compares it to the exact staged bytes, verifies completeness, then makes
the release public. No clobber or deletion of conflicting assets is permitted.

Recovery accepts existing immutable artifacts only after validating their own
checksums and comparing unpacked bytes, executable permissions and commit/
version/platform metadata to the staged build. Compression differences alone
are acceptable; binary differences are not. For a partial draft archive missing its checksum, validate the archive contents
against the staged build, retain its exact uploaded bytes, and add a checksum
for those bytes; never replace the archive just because compression differs.
Already-public releases are verified read-only by the publication command.
Never move tags or replace assets of a completed release.

After publication, run `python3 scripts/release.py published` with
`RELEASE_COMMIT` and `RELEASE_TAG` set. It requires a public release, the exact
tag commit and all eleven assets, validates every checksum and archive member,
and compares payloads with the preflight build. Both `ci.yml` and `release.yml`
must also succeed in the tag push context; branch evidence cannot replace tag
workflow verification. The installer consumes GitHub's latest release URL;
there is no independently published update feed.

## Preserved partial 0.4.4 release

The unsuccessful v0.4.4 tag run 35319300040 left draft release 391285344 with
only install.sh (asset 572072007). Its tag remains at
ea1b57d0d86a242d2837f8b6a768d8ea0a04ae9e. Preparation preserves that tag and
asset; v0.4.5 includes the repair on a new commit. Do not resume the old workflow
or treat that incomplete draft as a completed release. Draft discovery uses
the authenticated paginated release list and GraphQL when REST tag lookup
returns 404; upload
readback and final completeness checks use the known release ID.
