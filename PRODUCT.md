# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

static HTML/CSS, one file, hosted on GitHub Pages (https://adhishtanaka.github.io/thir/, from the repo's docs/ folder). The OS itself is not a web product; this record covers its website.

## Users

Developers and OS hobbyists who like small operating systems, Rust and Linux internals, and want to try thir in a VM in a few minutes. Second audience: people who look at it as the author's portfolio work and want to see what was built and how.

## Product Purpose

thir is a small operating system: the unmodified Linux 6.12 kernel plus a userspace written from scratch in Rust (init, login, shell, a framebuffer tiling desktop called den, apps). The author built it to learn how an OS works. The website explains what it is, shows it running, and gets visitors to a download or the source.

## Positioning

Not a Linux distribution in the usual sense: no GNU tools, no package manager, no systemd. Only the kernel is borrowed; everything a user sees and touches above it is thir's own Rust code. The AI is optional and always asks before it acts.

## Operating Context

Runs in VirtualBox (Apple Silicon and x86_64), UTM/QEMU, or a real PC from a USB stick (UEFI). 2 GB RAM, 2 CPUs. Releases on GitHub with .ova and .img.xz files.

## Capabilities and Constraints

Desktop of tiling panes (den), shell nsh with Tab and history, built-in apps (notes, calendar, contacts, clock, calculator, settings, music/video player, picture viewer), editor, find, top, a WebKit browser that plays YouTube, voice recorder and camera (den asks before mic/camera), copy/paste, themes. Optional AI with `!` lines and [Y/n] confirmation, bring-your-own key. Wi-Fi does not work. Not tested on real PC hardware for 0.7.x. Not signed for Secure Boot.

## Brand Commitments

Name "thir", always lowercase. Calm navy (#08152b) and peach (#f0b27f) identity, pixel "thir" wordmark, Terminus bitmap font in the OS, peach bear logo (assets/logo.svg). Voice: plain, first person, honest about limits, no hype.

## Evidence on Hand

Demo video docs/thir.mp4 and GIF docs/thir.gif; screenshots docs/den*.png; logo assets/logo.svg/png; release v0.7.1 files; MIT license. No users, testimonials, benchmarks or press exist: never invent them.

## Product Principles

- Show the real system, not claims about it.
- Honest limits beside the features.
- Small and understandable beats complete.
- The user stays in control (the AI asks first).
