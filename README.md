# Change Log

## Requirements
- GitHub CLI recommended

## Usage
Ensure GitHub auth token is held within the `GITHUB_TOKEN` environment variable.
One approach is to use the GitHub CLI command as follows.
```
export GITHUB_TOKEN="$(gh auth token)"
```

Execute:
```
cargo run
```