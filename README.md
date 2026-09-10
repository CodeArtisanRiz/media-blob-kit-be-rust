# MediaBlobKit (Rust Backend)

[![Rust](https://img.shields.io/badge/rust-stable-brightgreen.svg)](https://www.rust-lang.org/)
[![Axum](https://img.shields.io/badge/axum-0.8-blue.svg)](https://github.com/tokio-rs/axum)
[![SeaORM](https://img.shields.io/badge/sea--orm-1.1-orange.svg)](https://www.sea-ql.org/SeaORM/)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)

**MediaBlobKit** is a high-performance async media storage and image processing microservice built with **Rust**, **Axum**, **SeaORM**, and **PostgreSQL**. It delivers secure multi-tenant file management, S3-compatible cloud storage integration, and background worker queues using PostgreSQL's `FOR UPDATE SKIP LOCKED`.

---

## 🚀 Key Features

* **Multi-Tenant Project Isolation**: Files and API Keys are isolated per project with clean bucket key partitioning (`{project_slug}-{uuid}/...`).
* **Asynchronous Image Processing Engine**: Automatic resizing, cropping, aspect-ratio preservation, and format conversion (**JPEG**, **WebP**, **AVIF**, **PNG**) powered by the native `image` crate.
* **Per-Upload Variant Selection**: Dynamically request specific image variants during upload via `POST /upload/image?variants=thumbnail,card` or fallback to default project variant configs.
* **Lock-Free Background Queue**: Distributed worker queue using `FOR UPDATE SKIP LOCKED` guarantees safe horizontal scaling across multiple instances without duplicate job execution.
* **Granular Quality Compression**: Customizable JPEG encoding quality per variant configuration (`quality: 1-100`).
* **Authentication & RBAC**:
  * **Password Hashing**: Argon2id.
  * **Tokens**: Short-lived JWT access tokens + SHA-256 hashed refresh tokens.
  * **API Keys**: Scoped project API keys (`x-api-key`) for file upload endpoints.
  * **Roles**: `Su` (Superuser), `Admin`, and `User`.
* **Structured Observability**: Built-in HTTP request logging and diagnostic tracing with `tracing` and `tracing-subscriber`.
* **OpenAPI 3.0 / Swagger UI**: Self-documenting interactive API documentation served live at `/swagger-ui`.

---

## 🏗️ Architecture & How It Works

```
                        +----------------------------+
                        |  Client / Application API  |
                        +--------------+-------------+
                                       |
                   +-------------------v-------------------+
                   |   Axum Router & Auth Middleware       |
                   |   (JWT Bearer / x-api-key Auth)       |
                   +-------------------+-------------------+
                                       |
                   +-------------------v-------------------+
                   |     Fast S3 Original Upload & DB       |
                   |  (Immediately returns public URL)     |
                   +-------------------+-------------------+
                                       |
                          +------------v------------+
                          |   PostgreSQL Job Queue  |
                          | (FOR UPDATE SKIP LOCKED)|
                          +------------+------------+
                                       |
            +--------------------------v--------------------------+
            | Async Worker Pool (Semaphore Concurrency Controlled) |
            +--------------------------+--------------------------+
                                       |
       +-------------------------------+-------------------------------+
       |                                                               |
+------v-----------------------+                     +-----------------v---------------+
| Image Processing (Resizing,   |                     | Upload Generated Variants to S3  |
| Format Conversion & Quality) |                     | & Update DB `variants_json`     |
+------------------------------+                     +---------------------------------+
```

1. **Upload Request**: Client sends file upload (`/upload/file` or `/upload/image`) authenticated via `x-api-key`.
2. **Immediate Storage**: Original file is immediately written to AWS S3 / MinIO, and a file record is stored in PostgreSQL with status `processing`.
3. **Background Job Queue**: An image processing job is inserted into the `jobs` table.
4. **Worker Processing**: Background worker thread(s) claim jobs using `FOR UPDATE SKIP LOCKED`, process image variants in parallel (`tokio::task::spawn_blocking`), upload processed variants to S3, and update `variants_json` with full public URLs.
5. **File Retrieval**: Clients query `GET /files` to obtain metadata and public variant URLs, or call `GET /files/:id/content?variant=thumb` to receive a 307 temporary redirect to an S3 presigned URL.

---

## ⚙️ Environment Variables

Create a `.env` file in the root directory:

```env
# Server Binding
HOST=0.0.0.0
PORT=3000

# Database
DATABASE_URL=postgres://postgres:password@localhost:5432/mediablobkit

# Security
JWT_SECRET=super_secret_jwt_key_at_least_32_chars_long

# AWS S3 / MinIO Configuration
AWS_REGION=us-east-1
AWS_ACCESS_KEY_ID=your_access_key
AWS_SECRET_ACCESS_KEY=your_secret_key
S3_BUCKET_NAME=mediablobkit-bucket
S3_ENDPOINT=http://localhost:9000   # Optional: Set for MinIO or custom S3 compatible providers

# Worker Configuration
WORKER_CONCURRENCY=4                # Number of parallel jobs per worker instance

# Optional Auto-Superuser Creation on Startup
SU_USERNAME=admin
SU_PASSWORD=securepassword123
```

---

## 💻 Local Setup & Execution

### Prerequisites

- **Rust** (latest stable)
- **PostgreSQL** (v14+)
- **S3 Storage** (AWS S3, MinIO, Cloudflare R2, or DigitalOcean Spaces)

### 1. Run Database Migrations

Apply database schema migrations:

```bash
cargo run -- migrate
```

To reset the database (re-run all migrations):

```bash
cargo run -- reset
```

### 2. Create Superuser

Create an initial admin account via CLI:

```bash
cargo run -- create-superuser --username admin
```

*(You will be interactively prompted for a secure password).*

### 3. Run Application Server

```bash
# Development mode (fast compile)
cargo run

# Production mode (optimized image processing speed)
cargo run --release
```

The server will listen on `http://localhost:3000` (or `HOST`:`PORT` configured in `.env`). Interactive Swagger UI documentation is available at `http://localhost:3000/swagger-ui`.

---

## 🐳 Running with Docker

You can deploy **MediaBlobKit** using Docker Compose (single command) or direct Docker commands.

### Option 1: Docker Compose (Recommended 1-Step Deployment)

Build and start the application container in detached mode with a single command:

```bash
docker compose up -d --build
```

To stop the application container:

```bash
docker compose down
```

### Option 2: Direct Docker CLI (2-Step Deployment)

If you prefer building and running manually without Docker Compose:

1. **Build Docker Image**:
   ```bash
   docker build -t media-blob-kit-be-rust .
   ```

2. **Run Docker Container**:
   ```bash
   docker run -d \
     --name media-blob-app \
     --env-file .env \
     -p 3000:3000 \
     --restart unless-stopped \
     media-blob-kit-be-rust
   ```

---

## 📡 API Overview

| Category | Endpoint | Method | Auth | Description |
| :--- | :--- | :--- | :--- | :--- |
| **General** | `/` | `GET` | Public | System status check |
| **Docs** | `/swagger-ui` | `GET` | Public | Interactive OpenAPI Documentation |
| **Auth** | `/auth/login` | `POST` | Public | Authenticate user & issue tokens |
| **Auth** | `/auth/refresh` | `POST` | Public | Obtain new access token via refresh token |
| **Auth** | `/auth/logout` | `POST` | Public | Revoke refresh token |
| **Auth** | `/auth/me` | `GET` | Bearer JWT | Fetch current user profile |
| **Users** | `/users` | `POST` / `GET` | Bearer (Su) | Create or list users (Paginated) |
| **Users** | `/users/{id}` | `DELETE` | Bearer (Su) | Delete user |
| **Projects** | `/projects` | `POST` / `GET` | Bearer JWT | Create or list user projects |
| **Projects** | `/projects/{id}` | `GET` / `PUT` / `DELETE` | Bearer JWT | Manage project settings & soft/hard delete |
| **Projects** | `/projects/{id}/keys` | `POST` / `GET` | Bearer JWT | Create or list project API keys |
| **Upload** | `/upload/file` | `POST` | API Key | Standard raw file upload to S3 |
| **Upload** | `/upload/image` | `POST` | API Key | Image upload with optional `?variants=thumb,card` query |
| **Files** | `/files` | `GET` | Bearer JWT | List project files (Paginated) |
| **Files** | `/files/{id}` | `GET` / `DELETE` | Bearer JWT | Get file details or hard delete file + S3 objects |
| **Files** | `/files/{id}/content` | `GET` | Bearer JWT | Redirect to S3 presigned URL (supports `?variant=name`) |
| **Jobs** | `/jobs` | `GET` | API Key | List background image processing jobs |
| **Jobs** | `/admin/jobs` | `GET` | Bearer JWT | System-wide job monitoring for Admins |

---

## 🧪 Testing

Run automated unit and integration tests:

```bash
cargo test
```

---

## 📊 Scaling & Performance Guidelines

| Hardware Spec | Minimum | Recommended |
| :--- | :--- | :--- |
| **CPU** | 1 vCPU | 2–4 vCPU (for heavy image processing) |
| **RAM** | 512 MB | 2 GB |
| **Disk** | 5 GB | 20 GB (for OS & temp storage) |

- **Parallel Worker Concurrency**: Adjust `WORKER_CONCURRENCY` in `.env` to match available CPU cores.
- **Horizontal Scaling**: You can spin up multiple app/worker containers pointing to the same PostgreSQL database and S3 bucket. PostgreSQL's `FOR UPDATE SKIP LOCKED` guarantees zero duplicate processing across instances.

---

## 📄 License

Copyright (C) 2025 CodeArtisanRiz. This project is licensed under the [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0).
