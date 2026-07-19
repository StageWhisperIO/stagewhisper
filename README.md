# StageWhisper Lite

StageWhisper Lite is a native macOS app that listens to your calls and transcribes them on your own machine. The audio never leaves your computer. You connect your own AI assistant, and it can follow the conversation and respond on your screen while you talk.

This repository is a public, read-only mirror of the Lite edition's source. It is published so anyone can read the code, audit what the app does, and build it from scratch. The full StageWhisper product, with cloud coaching, Playbooks, and tone analysis, lives at [stagewhisper.io](https://stagewhisper.io).

## Lite is a notetaker. Founders is a clairvoyant.

Lite listens and writes down what happened. That's useful, but it can't help you mid-sentence. You still hang up and realize the perfect answer ten seconds too late.

The [Founders Edition](https://stagewhisper.io/lite) works during the call. While you're still talking, it slips you Insights on a private overlay only you can see: the objection you already answered two calls ago, or the follow-up question you were about to forget.

![StageWhisper Founders Edition showing a live cue on screen during a call](https://stagewhisper.io/images/live-card.webp)

The coaching is only part of it. Founders keeps an encrypted Library of every call on your Mac and shares it with your AI over a local MCP server, so your assistant walks into the next call already knowing the deal. Let it read your screen (it asks first) and the doc or dashboard in front of you shows up in its suggestions. Write down how you actually run a call and it coaches you off your own playbook. A local model comes built in. None of this requires running your own assistant.

It's $99, once. A lifetime license and a seat in the Founders Circle.

[Take your Founder seat at stagewhisper.io/lite](https://stagewhisper.io/lite)

## What it does

- Captures your call audio and transcribes it locally, on-device. Nothing is sent to a server for transcription.
- Optionally captures your microphone too, so your own side of the call is in the transcript, labeled separately from the other party.
- Connects to an AI assistant you run yourself (Hermes, OpenClaw), so it can read the conversation and reply during the call.
- Keeps everything on a private overlay that the people on your call never see.

## Requirements

- macOS 14 (Sonoma) or later, Apple Silicon.
- Microphone and Screen Recording permissions, which the app requests on first launch.
- An AI assistant to connect to, if you want live responses. See the [assistant guide](https://docs.stagewhisper.io/agents/index).

## Install

Most people should download the signed StageWhisper Lite build from [stagewhisper.io/lite](https://stagewhisper.io/lite) rather than build it. The steps below are for reading the code and building it yourself.

## Build from source

You need [Rust](https://rustup.rs), [Node.js](https://nodejs.org) 20 or later, [pnpm](https://pnpm.io), and the Xcode Command Line Tools.

```bash
pnpm install
pnpm tauri dev      # run it locally
pnpm tauri build    # produce a .app and .dmg
```

The build compiles a small Swift bridge for the native control bar. If Xcode is missing, that step is skipped and the app falls back to a plain HTML control bar; everything else still works.

## Layout

- `src/`: the React front end (control bar, session panel, settings).
- `src-tauri/`: the Rust back end, covering audio capture, on-device transcription, the relay that talks to your assistant, and the Tauri shell.

## Privacy

Transcription runs entirely on your machine. Your call audio is never uploaded. The only thing that leaves your computer is the text you choose to send to the assistant you connected, and that goes straight to your assistant. It does not pass through us.

## Notes

This mirror is generated from the StageWhisper monorepo, so it does not take pull requests directly. If you find a bug or have a question, email [piotr.m@stagewhisper.io](mailto:piotr.m@stagewhisper.io).

Documentation lives at [docs.stagewhisper.io](https://docs.stagewhisper.io).

## Lite is a notetaker. Founders is a clairvoyant.

Lite listens and writes down what happened. That's useful, but it can't help you mid-sentence. You still hang up and realize the perfect answer ten seconds too late.

The [Founders Edition](https://stagewhisper.io/lite) works during the call. While you're still talking, it slips you Insights on the same private overlay: the objection you already answered two calls ago, or the follow-up question you were about to forget.

![StageWhisper Founders Edition showing a live cue on screen during a call](https://stagewhisper.io/images/live-card.webp)

The coaching is only part of Founders. Let it read your screen (it asks first) and the doc or dashboard in front of you shows up in its suggestions. Write down how you actually run a call and it coaches you off your own playbook. A local model comes built in, so none of this requires running your own assistant.

It's a one-time payment, lifetime license. Consider buying it for you and your team members if you want to support this project.

[Take your Founder seat at stagewhisper.io/lite](https://stagewhisper.io/lite)
