# AGENTS.md — regolith-powerd (Regolith Fedora port)

This is the Port Org fork of `regolith-linux/regolith-powerd`. Fedora packaging for
Upstream branch `master` lives on `fedora/master`.
Owning Task: 02-08.

## Branches
- Work only on `fedora/master`. Never commit to `master`
  or any other Upstream-named branch.
- Pull requests target `fedora/master` in this fork.

## Where things are
- The spec is `<source-package>.spec` at the repo root, beside `debian/`.
- Style guide: https://github.com/regolith-linux-fedora/voulage/blob/fedora/main/docs/fedora/spec-style-guide.md
- Override Exceptions registry: https://github.com/regolith-linux-fedora/voulage/blob/fedora/main/docs/fedora/override-exceptions.md
- Workflows: https://github.com/regolith-linux-fedora/actions/blob/fedora/main/docs/fedora-workflows.md

## Done means Green
Green is the only completion bar: the CI build, rpmlint and the install test
pass on every non-canary Target. A Local Loop pass is not Green.

## Human gates (agents never do these)
- No pull requests, issues or comments in `regolith-linux` (Upstream).
- No contact with Fedora (Bugzilla, mailing lists, Matrix, package reviews).
- No publish to the stable COPR project.

## Commits
- Source Changes and Packaging Changes go in separate commits.
- Packaging commits start with `fedora:`; Source Changes follow Upstream's style.
- Say in each pull request which kind every commit is.

## Knowledge
Record durable facts in the project wiki (contributors only).

## Local Loop
1. `rpmbuild -bs <source-package>.spec`
2. `mock -r fedora-<N>-<arch> --addrepo=<COPR unstable repo URL> <srpm>`
3. `rpmlint` on the spec and the built RPMs
4. Install-test in `podman run --rm -it fedora:<N>`
5. Optionally, the Session Test.
