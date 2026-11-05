# Testing the local LLM (BYO AI) feature

The on-device LLM runs through `sw-local-llm`, which drives a bundled llama.cpp
`llama-server` sidecar (Metal) over a localhost OpenAI-compatible streaming API. The
crate is GGUF-only. Both `stagewhisper-desktop` and `desktop-free` share it.

## Prerequisite: fetch the sidecar binary

The sidecar (`llama-server` + its dylibs) is not committed. Fetch it into both apps'
`src-tauri/sidecar/llama/` (gitignored):

```
scripts/fetch-llama-sidecar.sh
```

Override the build with `LLAMA_BUILD=bNNNN scripts/fetch-llama-sidecar.sh` (default
`b9544`, macos-arm64). The same dir is what `tauri.conf.json` bundles into
`Resources/llama/` for release.

## Crate-level smoke test (real download + inference)

Downloads a small ungated GGUF and runs real inference end to end through the sidecar.
Point `SW_LLAMA_DIR` at the fetched binary dir:

```
cd stagewhisper-desktop/src-tauri/sw-local-llm
SW_LLAMA_DIR=$PWD/sidecar/llama \
  cargo run --example local_infer -- "Qwen/Qwen2.5-0.5B-Instruct-GGUF" "Say hi."
```

The example above pulls a tiny Qwen GGUF for a fast smoke test; pass the curated
`gemma-4-e2b-it` id to exercise the real ~2.6GB default. The model downloads to
`~/Library/Application Support/com.stagewhisper.app/models/llm/` and is reused. Expected:
a coherent answer streamed to stdout and a final `OK: generated <n> chars` line.

## Unit tests

```
cargo test -p sw-local-llm        # registry resolve, GGUF selection, path-traversal guard, hf-cache
cargo test --lib                  # in each app's src-tauri: dispatcher routing + assistant gating
```

`dispatcher.rs` covers `select_transport_v2` (local-primary, external-online,
external-offline+ready fallback, offline+not-ready) and `assistant_unavailable`
heartbeat thresholds.

## Manual UI end-to-end

1. Run `scripts/fetch-llama-sidecar.sh` once (dev resolves the sidecar from
   `src-tauri/sidecar/llama`).
2. `npm run tauri dev` in the app directory.
3. Onboarding (stagewhisper-desktop): pick **Use a local model**, download the
   recommended Gemma 4 E2B (ungated, no token) or the larger Gemma 4 E4B. For any other
   model, use the custom Hugging Face field with a GGUF repo id (add an access token for
   gated/private repos), or **Use a model from your computer** to point at a folder with a `.gguf`.
4. Settings -> Local Model: confirm `Installed`, toggle **Use as primary responder**.
5. Send a chat message and confirm the reply streams in from the local model
   (`chat-message-created` events, no network).
6. Auto-fallback: pair an external assistant, then stop its heartbeat (or disconnect).
   With a model installed, the next message routes to the local model automatically and
   routes back once the assistant is online again.

The curated defaults are Unsloth dynamic quants of Google's QAT bases:
`unsloth/gemma-4-E2B-it-qat-GGUF` file `gemma-4-E2B-it-qat-UD-Q4_K_XL.gguf` (~2.6 GB, default),
`unsloth/gemma-4-E4B-it-qat-GGUF` (~4.2 GB), and `unsloth/gemma-4-12B-it-qat-GGUF` (~6.7 GB,
needs ~16 GB RAM). All ungated and loaded directly by llama.cpp. Each curated entry pins its
exact `.gguf` filename (the multimodal `mmproj` is never fetched). Custom Hugging Face repos
must be GGUF; for those the downloader picks the Q4_K_M shard group.
