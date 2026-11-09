# Releasing Taiwalog

## Repository rename gate

Complete the GitHub repository rename before publishing the first Taiwalog
release. Repository administration owns this one-time operation.

1. Merge the Taiwalog identity change into the default branch of
   `choplin/agentlog`.
2. Rename the GitHub repository to `choplin/taiwalog`.
3. Update the local `origin` URL to `git@github.com:choplin/taiwalog.git` or the
   equivalent HTTPS URL.
4. Verify that `https://github.com/choplin/taiwalog` resolves and that the old
   `https://github.com/choplin/agentlog` URL redirects to it.
5. Verify that the default branch, Actions workflows, release permissions, and
   Nix flake URL are available under the new repository name.
6. Run the release dry-run and confirm that cargo-dist plans `taiwalog`
   artifacts and the `taiwalog` Homebrew formula before creating a tag.

Do not publish the Taiwalog crate, tag, GitHub release, or Homebrew formula
while the canonical repository URL still returns an error.
