#!/usr/bin/env bash
# Prints the Homebrew formula for a release, using the .sha256 files that
# release-assets.yml uploads. Usage: homebrew-formula.sh v0.1.0
set -euo pipefail

tag="${1:?usage: homebrew-formula.sh <tag>}"
repo="${GITHUB_REPOSITORY:-darwin-luque/usagi}"
base="https://github.com/$repo/releases/download/$tag"

sha() {
  gh release download "$tag" --repo "$repo" --pattern "usagi-$tag-$1.tar.gz.sha256" --output - | cut -d ' ' -f 1
}

cat <<EOF
class Usagi < Formula
  desc "Show the usage limits of your AI agent harnesses"
  homepage "https://github.com/$repo"
  license "MIT"

  on_macos do
    on_arm do
      url "$base/usagi-$tag-aarch64-apple-darwin.tar.gz"
      sha256 "$(sha aarch64-apple-darwin)"
    end
    on_intel do
      url "$base/usagi-$tag-x86_64-apple-darwin.tar.gz"
      sha256 "$(sha x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$base/usagi-$tag-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "$(sha aarch64-unknown-linux-gnu)"
    end
    on_intel do
      url "$base/usagi-$tag-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "$(sha x86_64-unknown-linux-gnu)"
    end
  end

  def install
    bin.install "usagi"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/usagi --version")
  end
end
EOF
