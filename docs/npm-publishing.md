# Publishing to npm

The public package is `@brokkai/muse-acp`; its executable is `muse-acp`.
`npm/package.json` is a private packaging template. The packager takes the
version from `Cargo.toml`, removes `private`, and bundles the five verified
native release binaries with a small Node.js launcher. It includes no npm
dependencies or install scripts. Do not publish the template directory.

## Release workflow

`.github/workflows/release.yml` builds and tests the native binary and npm
launcher on every supported platform. Its publisher job verifies all release
archives, packs and installs the npm tarball for a smoke test, and publishes
on an explicit `v*` tag push. GitHub Releases is published before npm. Branch
pushes and manual workflow dispatches only validate the npm package.

The publisher uses Node.js 24, npm 11.19.0, and `id-token: write`. npm obtains
short-lived publishing credentials through GitHub OIDC; no npm token secret
is needed. Trusted publishing also generates npm provenance for the public
repository. A rerun skips an existing npm version only if its tarball integrity
matches exactly; different contents require a new version. Prereleases use
the `next` dist-tag. After publishing, verification waits up to five minutes
for npm's registry metadata to become available without publishing again.

## First publication and trusted publisher setup

The package must exist before configuring its trusted publisher. Use an npm
account with publishing access to the `@brokkai` scope and 2FA enabled.
`npm trust` is built into the installed npm CLI; no Cargo plugin is required.

For a release with a clean checkout at its tag and verified release assets in
`dist/`, run:

```sh
npm login --registry=https://registry.npmjs.org/
python3 scripts/npm_release.py pack
npm publish ./npm-dist/brokkai-muse-acp-<version>.tgz --access public --ignore-scripts
npm trust github @brokkai/muse-acp --repo BrokkAi/muse-acp --file release.yml --allow-publish
npm trust list @brokkai/muse-acp --json
```

Replace `<version>` with the Cargo version. Complete npm's browser/2FA prompts
when requested. The trusted publisher must use owner `BrokkAi`, repository
`muse-acp`, and workflow filename `release.yml`, with direct publishing allowed
and no environment restriction (the workflow does not declare an environment).
Merge the workflow before the next release tag. A successful tag publication
is the end-to-end verification that the trust configuration works.

To bootstrap from an older GitHub release that predates the npm packager,
check out that tag in a separate clean clone or worktree, download its release
assets, and invoke the new packager by absolute path from that checkout:

```sh
gh release download v0.5.0 --repo BrokkAi/muse-acp --dir dist --pattern 'muse-acp-*'
python3 /path/to/npm-enabled-checkout/scripts/npm_release.py pack
```

This keeps the native version, commit, checksums, and license files tied to the
released tag while using the new launcher and package metadata. The packager
rejects missing platforms, corrupt archives, and mismatched release metadata.

See npm's [trusted publishing documentation](https://docs.npmjs.com/trusted-publishers/)
and [`npm trust` reference](https://docs.npmjs.com/cli/v11/commands/npm-trust/).
