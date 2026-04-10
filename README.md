# clipygo-plugin-obsidian

Obsidian vault plugin for [clipygo](https://github.com/it-atelier-gn/clipygo). Send clipboard content directly to your Obsidian vault.

## Targets

| Target | Description |
|--------|-------------|
| **Daily Note** | Append to today's daily note |
| **Inbox** | Append to a configurable inbox note |
| **New Note** | Create a new note (first line becomes the filename) |

## Configuration

| Setting | Default | Description |
|---------|---------|-------------|
| Vault Path | Auto-detected | Path to your Obsidian vault directory |
| Daily Notes Folder | `Daily Notes` | Subfolder for daily notes |
| Daily Notes Format | `%Y-%m-%d` | Date format for daily note filenames |
| Inbox Note | `Inbox.md` | Filename for the inbox note |
| Append Template | `\n---\n*{timestamp}*\n{content}\n` | Template with `{timestamp}` and `{content}` placeholders |

The vault path is auto-detected from Obsidian's config file on first run.

## Install

Download the binary for your platform from [Releases](https://github.com/it-atelier-gn/clipygo-plugin-obsidian/releases), then add it in clipygo Settings → Plugins.
