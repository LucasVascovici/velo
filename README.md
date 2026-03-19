<p align="center">
  <!-- Latest Release Version -->
  <img src="https://img.shields.io/github/v/release/LucasVascovici/velo?color=orange&logo=github" />
  <!-- Continuous Integration (Push/PR Tests) -->
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/ci.yml/badge.svg" />
  <!-- Release Build & Regression Tests -->
  <img src="https://github.com/LucasVascovici/velo/actions/workflows/release.yml/badge.svg" />
  <!-- License -->
  <img src="https://img.shields.io/github/license/LucasVascovici/velo?color=blue" />
</p>

# 🚀 Velo

**Velo** is a blazingly fast, safety-first version control system built in Rust. 

Git often feels like a beautifully engineered engine with a dashboard from the 1970s. Velo was born from a simple question: *"What if we kept what Git does great (snapshots, branching) and fixed everything that makes it unintuitive?"*

> **✨ Note on Vibe Coding:** This project was fully "vibe coded" for fun. It combines high-level human intuition with a modern tech stack (BLAKE3, Rayon, Zstd, SQLite) to see how far we can push local version control in a weekend.

---

## 🏎️ Why Velo?

- **Implicit Staging:** No more `git add`. If you see it on your disk, Velo saves it.
- **Side-car Conflicts:** Merge conflicts are saved to `.conflict` files. Your original code stays runnable and valid during the merge process.
- **Safety First:** Velo blocks you from switching branches or restoring if you have unsaved changes. No more `reflog` panic.
- **Recursive Ancestry:** Beautifully aligned logs with a `-->` pointer showing your exact position in a branching timeline.
- **Parallel Core:** Powered by **Rayon**, Velo hashes and compresses thousands of files simultaneously across all CPU cores.

---

## 📊 Performance at Scale
*Tested on a monorepo with 1,000+ files and 200+ commits:*

| Command | Avg Latency | Tech Stack |
| :--- | :--- | :--- |
| **Status** | ~140ms | Parallel BLAKE3 Hashing |
| **Save** | ~200ms | SQLite Transactions + Zstd |
| **Switch** | ~900ms | Optimized I/O Overwrite |

---

## 📦 Quick Installation

### Unix (Linux & macOS)
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/your_username/velo/raw/main/install.sh | sh
```

### Windows
1. Download the latest `velo-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/your_username/velo/releases).
2. Extract `velo.exe` and add it to your system PATH.

---

## 🛠️ The 60-Second Workflow

```bash
# Start a project
velo init

# Make a change
echo "hello world" > app.py
velo save "Initial commit"
velo tag v1.0

# Branch out safely
velo switch feature-ui
echo "print('vibe')" >> app.py
velo status
velo diff

# Oops?
velo undo

# Time Travel
velo logs --all
velo restore v1.0 --force
```

---

## 🏗️ Architecture
Velo isn't a flat-file index. It’s a **high-integrity database**.
- **Engine:** BLAKE3 for collision-proof, fast hashing.
- **Storage:** Zstd compression for minimal disk footprint.
- **Metadata:** SQLite (WAL mode) for structured, searchable history.
- **Concurrency:** Rayon for multi-threaded performance.

---

## 📜 License
MIT / For fun. Built with 🦀 by Lucas Vascovici.