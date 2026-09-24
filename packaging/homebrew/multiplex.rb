cask "multiplex" do
  version "0.0.4"
  sha256 "715a7eac27f299ad3c95a187970bce7cbc4aecfa79dec689d1e8c6913105bbc7"

  url "https://github.com/millionrust/multiplex/releases/download/v#{version}/Multiplex-macos-universal.zip",
      verified: "github.com/millionrust/multiplex/"
  name "Multiplex"
  desc "Terminals and screens, wherever you are"
  homepage "https://github.com/millionrust/multiplex"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The app updates itself from the same releases, so `brew upgrade` must leave it alone:
  # otherwise Homebrew reinstalls over a version the app already replaced.
  auto_updates true
  depends_on macos: ">= :ventura"

  app "Multiplex.app"

  # Removing the app is not enough on its own: a background listener may be registered to start
  # at login, and terminal profiles in other apps point at this copy. The app takes both out
  # itself, and launchctl catches a service left behind by an older build.
  uninstall quit:      "com.millionrust.multiplex",
            script:    {
              executable:   "#{appdir}/Multiplex.app/Contents/MacOS/multiplex",
              args:         ["--uninstall-cleanup"],
              must_succeed: false,
            },
            launchctl: "com.millionrust.multiplex.controller-service"

  # Everything a person would lose is in the one directory: saved hosts, vaults, pinned host
  # keys, snippets, session history, durable sessions, and diagnostics. Only `brew zap` removes
  # it. Secrets live in the login keychain and are deliberately left alone.
  zap trash: [
    "~/Library/Application Support/multiplex",
    "~/Library/LaunchAgents/com.millionrust.multiplex.controller-service.plist",
    "~/Library/Logs/multiplex",
    "~/Library/Saved Application State/com.millionrust.multiplex.savedState",
  ]

  caveats <<~CAVEATS
    Multiplex is not signed with an Apple Developer ID yet, so macOS asks before opening it the
    first time: right-click the app and choose Open, then Open again. It also asks to use its
    own keychain items after each update, because every unsigned build looks like a new app to
    the keychain. Choose Always Allow.
  CAVEATS
end
