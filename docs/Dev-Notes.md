# Development Notes

Development notes on various topics.

## 1. Filtering

devports reads your listening TCP ports with `lsof`.

A port is listed when its process is started by a runner like `npm run dev` or runs on a runtime like Node or Python.

Mac apps and coding agents are skipped.

The rules live in `src/discovery/classification.rs`.
