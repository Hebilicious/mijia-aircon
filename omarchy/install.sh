#!/usr/bin/env bash
# Install the Omarchy bar widget: copy the plugin into the user plugin
# directory, put it on the bar (before the battery when there is one), and
# restart the shell. The shell's hot reload does not pick up QML edits to an
# already-loaded plugin, so the restart is needed on every update.
set -euo pipefail

id="xiaomi.aircon"
source_dir="$(cd "$(dirname "$0")" && pwd)"
target_dir="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins/$id"
shell_config="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/shell.json"

if ! command -v omarchy >/dev/null 2>&1; then
  echo "omarchy is not installed; this widget is for the Omarchy shell" >&2
  exit 1
fi
if ! command -v aircon >/dev/null 2>&1; then
  echo "warning: aircon is not on PATH; install it first (see the README's Install section)" >&2
fi

mkdir -p "$target_dir"
cp "$source_dir/manifest.json" "$source_dir/Panel.qml" "$target_dir/"
echo "Installed $target_dir"

if ! grep -q "\"$id\"" "$shell_config" 2>/dev/null; then
  if grep -q '"omarchy.power"' "$shell_config" 2>/dev/null; then
    omarchy bar put "$id" --before omarchy.power
  else
    omarchy bar put "$id"
  fi
fi

omarchy restart shell >/dev/null
