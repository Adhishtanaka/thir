<p align="center"><img src="assets/logo.png" width="140" alt="thir logo"></p>

# thir

thir is a tiny operating system I built for fun and to learn how one works from the inside. It uses the Linux kernel, and everything on top of it is my own code, written in Rust, apart from a few well-known libraries for things like password hashing and encryption. That covers the startup program, the shell, the login, the desktop and the small apps.

The whole system is about 10 MB and boots in a couple of seconds.

![thir running in VirtualBox: first boot, help, files, editor, split screen](docs/demo.gif)

Full video: [docs/demo.mp4](docs/demo.mp4) (about 2 minutes, recorded in VirtualBox on a Mac).

## What you get

- A login with your own user and password. Passwords are stored as a slow, salted hash, never as plain text.
- A desktop of terminal panes that you can split side by side, with a small bear wallpaper.
- Your files are saved on a disk and are still there after a restart.
- A shell with Tab completion and history (Up and Down arrows).
- A few apps: `edit` (a simple editor, also called `nano`), `find`, `top` (CPU and memory use) and `neofetch`.
- `curl`, for downloading web pages and files over HTTP and HTTPS.
- `agent`, an AI chat in the terminal that can also build small apps and do file jobs for you.

## How it works

When you turn it on, the computer's firmware loads the Linux kernel. The kernel starts `init`, my first program, and from there everything is thir's own Rust code.

```mermaid
block-beta
  columns 6
  l0["Your apps"] built["apps agent wrote for you: weather, calc, todo … each in a Lua sandbox"]:5
  l1["Apps"] edit["edit / nano"] find["find"] top["top / neofetch"] net["curl"] agent["agent: AI chat and app builder"]
  l2["Shell"] shell["nsh: commands, Tab completion, history"]:5
  l3["Session"] login["login: password check"]:2 den["den: tiling desktop"]:2 dhcp["dhcp: network"]
  l4["Startup"] init["init: the first program, starts and watches everything"]:5
  l5["Kernel"] kernel["Linux 6.12: screen, keyboard, disk and network drivers"]:4 disk[("disk: your files")]
  l6["Hardware"] hw["UEFI firmware on VirtualBox, QEMU or a real PC"]:5

  classDef label fill:transparent,stroke:transparent,color:#8f8b89
  classDef user fill:#2c2b2d,stroke:#cfc4be,color:#e3dfdc
  classDef sys fill:#222123,stroke:#8f8b89,color:#e3dfdc
  classDef core fill:#161617,stroke:#6a6668,color:#e3dfdc
  class l0,l1,l2,l3,l4,l5,l6 label
  class edit,find,top,net,agent,shell,built user
  class login,den,dhcp,init sys
  class kernel,hw,disk core
```

Read it from the bottom up: each layer starts the one above it. The lighter boxes (your shell and apps) run as you, so they can only touch your own files. The apps `agent` writes sit on top and get even less: no files at all, only the websites you allowed. The darker boxes run the system.

### The "LLM as OS" part

This is the bit I'm most curious about. Normally you go find an app that does roughly what you want. In thir you just say it: `agent build "a weather app"`, and an AI model writes a small app for you on the spot. It plans the app, writes it in Lua, and thir checks the code and test-runs it offline before you ever see it. If the test fails, the model gets the error and tries again.

You can also ask for plain file jobs: `agent do "download this page and put the names in names.txt"`. agent shows you exactly what it's going to write, move or download, and nothing happens until you press `y`. It can't delete anything or run programs.

It remembers small things you tell it, like `agent remember city: Colombo`, so the weather app opens on your city without asking. That memory is one file only you can read.

The model never gets real power. Apps run in a sandbox with no access to your files or keys, and they only reach the websites you said yes to. You bring your own API key (Anthropic, OpenAI, Gemini, DeepSeek, or a local Ollama). It's still rough: small models sometimes need a retry, and a two-minute wait for an app is long. But watching a computer write itself a tool you asked for is pretty fun.

## Try it

Download the files from the [latest release](https://github.com/Adhishtanaka/thir/releases/latest).

**VirtualBox on an Apple Silicon Mac (easiest).** Get `thir-vbox-arm64.ova`, open VirtualBox, choose File > Import Appliance, then press Start. On the first boot, thir asks you to make a user name and a password.

**VirtualBox on Windows, Linux or an Intel Mac.** Get `thir-vbox-x86_64.ova` and import it the same way (File > Import Appliance), then press Start. The appliance is already set to use EFI firmware, so you do not need to change any settings.

**A real PC from a USB stick.** Get `thir-x86_64-uefi.img.xz` and write it to a USB stick:

```
diskutil list                      # find the USB stick, e.g. /dev/disk4
diskutil unmountDisk /dev/diskN
xzcat thir-x86_64-uefi.img.xz | sudo dd of=/dev/rdiskN bs=4m
diskutil eject /dev/diskN
```

Be careful here. `dd` erases the whole disk you give it and does not ask first. If you type the wrong number, you can wipe your own Mac. Check `diskutil list` twice.

Then, on the PC, turn off Secure Boot in the firmware settings, open the boot menu (usually F12, F8, Esc or Del) and pick the USB stick. thir does not touch the PC's own disks. Wired network works on most PCs. Wi-Fi does not work yet.

**UTM or QEMU on ARM.** Use `thir-arm64-uefi.img.xz` as a UEFI disk.

## Using it

| Keys | What they do |
|---|---|
| Option + Enter (or Cmd + Enter) | Split the screen into a new pane |
| Option + arrow keys | Move between panes |
| Option + q | Close a pane |
| Option + f | Make a pane full screen |

Some commands to start with: `help`, `ls`, `mkdir notes`, `edit notes/todo.txt`, `find . -name '*.txt'`, `top`, `neofetch`, `date`. Type `help` to see them all.

## Build it yourself

You need a Mac with Docker and QEMU (`brew install qemu`).

```
./build.sh          # build everything
./run.sh gui        # start it in a window
```

## License

thir's own code is under the [MIT License](LICENSE). The Linux kernel inside the images is GPL-2.0 (unmodified 6.12.117, [source](https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.117.tar.xz)), and the Terminus font is under the SIL Open Font License. rustls and ring (Apache-2.0/MIT/ISC) encrypt web connections; root certificates from webpki-roots (CDLA-Permissive-2.0). Lua 5.4 (MIT) is included too.
