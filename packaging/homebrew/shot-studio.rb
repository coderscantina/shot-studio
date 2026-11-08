# Copy to Casks/shot-studio.rb in coderscantina/homebrew-tap. After each release, set version
# and sha256 from the release workflow's run summary.
cask "shot-studio" do
  version "26.11.8,0000000"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/coderscantina/shot-studio/releases/download/v#{version.csv.first}-#{version.csv.second}/Shot-Studio-#{version.csv.first}.dmg"
  name "Shot Studio"
  desc "Record and edit developer videos, with the edit as a text file in git"
  homepage "https://github.com/coderscantina/shot-studio"

  livecheck do
    url :url
    regex(/^v(\d+(?:\.\d+)+)-(\h+)$/i)
    strategy :github_latest do |json, regex|
      match = json["tag_name"]&.match(regex)
      next if match.blank?

      "#{match[1]},#{match[2]}"
    end
  end

  depends_on arch: :arm64
  depends_on macos: ">= :tahoe"

  app "Shot Studio.app"
  binary "#{appdir}/Shot Studio.app/Contents/MacOS/shot"

  zap trash: [
    "~/Library/Application Support/Shot Studio",
    "~/Library/Caches/com.coderscantina.shotstudio",
    "~/Library/Preferences/com.coderscantina.shotstudio.plist",
    "~/Library/Saved Application State/com.coderscantina.shotstudio.savedState",
  ]
end
