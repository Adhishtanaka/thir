<p align="center"><img src="assets/logo.png" width="120" alt="thir logo"></p>

# thir

thir is a small operating system I wrote to learn how one works. It boots the Linux kernel, and everything after that is my own Rust code.

<p align="center"><img src="docs/thir.gif" alt="thir: login, AI setup, neofetch, notes, the browser, an AI-built weather app and the video player"></p>

The GIF runs at double speed. [Watch the full video with sound](https://github.com/Adhishtanaka/thir/raw/master/docs/thir.mp4) (1 minute).

## What's in it

You get a desktop of panes that you split side by side, a shell, and a login. There are small apps for notes, a calendar, contacts, a clock and a calculator. A browser (YouTube plays) is included, plus a music and video player, a picture viewer, a voice recorder and a camera.

The AI is optional. Type `!` and say what you want, like `! move notes into docs` or `! make me a weather app`. thir shows you what it will do and waits for your yes. You bring your own key, or you pick "no AI" when you first log in.

## How it fits together

```mermaid
block-beta
  columns 6
  l0["Your apps"] built["apps the AI built for you (optional)"]:5
  l1["Apps"] edit["edit"] find["find"] top["top"] web["web, play, view, rec, camera"] agent["the AI (optional)"]
  l2["Shell"] shell["nsh: commands, Tab, history, ! lines"]:5
  l3["Session"] login["login"]:2 den["den: desktop"]:2 dhcp["dhcp"]
  l4["Startup"] init["init: starts and watches everything"]:5
  l5["Kernel"] kernel["Linux 6.12"]:4 disk[("your files")]
  l6["Hardware"] hw["UEFI: VirtualBox, QEMU or a real PC"]:5

  classDef label fill:transparent,stroke:transparent,color:#8fa3c2
  classDef user fill:#1d3557,stroke:#8b9bb4,color:#e8eefa
  classDef opt fill:#1d3557,stroke:#8b9bb4,color:#e8eefa,stroke-dasharray:5 4
  classDef sys fill:#142744,stroke:#6f86ad,color:#e8eefa
  classDef core fill:#0a1424,stroke:#4a5f85,color:#e8eefa
  class l0,l1,l2,l3,l4,l5,l6 label
  class edit,find,top,web,shell user
  class built,agent opt
  class login,den,dhcp,init sys
  class kernel,hw,disk core
```

Read it from the bottom up. Each layer starts the one above it. The two boxes marked "optional" are the AI. thir works without them.

## Try it

Download a file from the [latest release](https://github.com/Adhishtanaka/thir/releases/latest):

| File | For |
|---|---|
| `thir-vbox-arm64.ova` | VirtualBox on an Apple Silicon Mac |
| `thir-vbox-x86_64.ova` | VirtualBox on Windows, Linux or an Intel Mac |
| `thir-arm64-uefi.img.xz` | UTM or QEMU on ARM |
| `thir-x86_64-uefi.img.xz` | a real PC, from a USB stick |

Give the VM 2 GB of memory and 2 CPUs. `INSTALL.txt` in the release has the USB steps. Be careful with `dd`, because it erases the disk you name without asking.

## Keys

| Keys | What they do |
|---|---|
| Option + Enter | new pane |
| Option + arrows | move between panes |
| Option + f / q | full screen / close a pane |
| Ctrl + Shift + C / V | copy / paste (Cmd + C / V works too) |

Type `help` to see the rest.

## Build it

You need a Mac with Docker and QEMU (`brew install qemu`).

```
./build.sh                # the first build compiles WebKit and takes hours
./run.sh gui
THIR_SMALL=1 ./build.sh   # tiny version without the browser and media apps
```

## License

My code is MIT ([LICENSE](LICENSE)). The Linux kernel is GPL-2.0 ([source](https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.117.tar.xz)). WebKit, GStreamer, glib, libsoup, GnuTLS and ffmpeg are LGPL. The release notes list the source links.
