# Storage & Imaging Guide

File storage with local + S3 backends, multipart uploads, and a chainable image processing pipeline.

---

## Quick Start

```rust
// Upload a file from a handler
async fn upload(State(app): State<AppContext>, mut multipart: Multipart) -> Result<impl IntoResponse> {
    let form = MultipartForm::from_multipart(&mut multipart).await?;
    let file = form.file("avatar")?;
    let stored = file.store(&app, "avatars").await?;
    Ok(Json(json!({ "url": stored.url })))
}
```

---

## Config

```toml
# config/storage.toml
[storage]
default = "local"

[storage.disks.local]
driver = "local"
root = "storage/app"
url = "/storage"                    # public URL prefix
visibility = "private"              # "public" or "private"

[storage.disks.s3]
driver = "s3"
bucket = "my-bucket"
region = "ap-southeast-1"
key = "AKIA..."
secret = "..."
# endpoint = "https://..."         # custom endpoint for MinIO, R2, etc.
# url = "https://cdn.example.com"  # public URL prefix
# use_path_style = false
visibility = "public"
```

---

## Handling Uploads

### MultipartForm

Extract files and text fields from multipart requests:

```rust
async fn create_post(
    State(app): State<AppContext>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse> {
    let form = MultipartForm::from_multipart(&mut multipart).await?;

    let title = form.text("title").unwrap_or("Untitled");
    let cover = form.file("cover")?;

    let stored = cover.store(&app, "posts/covers").await?;

    Ok(Json(json!({
        "title": title,
        "cover_url": stored.url,
        "cover_size": stored.size,
    })))
}
```

### UploadedFile Methods

```rust
let file: &UploadedFile = form.file("avatar")?;

// Store with auto-generated name (UUIDv7 + original extension)
let stored = file.store(&app, "avatars").await?;
// → avatars/01912a4b-7c8d-7000-abcd-ef1234567890.jpg

// Store with custom name
let stored = file.store_as(&app, "avatars", "profile.jpg").await?;
// → avatars/profile.jpg

// Store on a specific disk
let stored = file.store_on(&app, "s3", "avatars").await?;

// Store on specific disk with custom name
let stored = file.store_as_on(&app, "s3", "avatars", "profile.jpg").await?;

// File metadata
file.original_name;    // Option<String> — "photo.jpg"
file.content_type;     // Option<String> — "image/jpeg"
file.size;             // u64 — bytes
file.original_extension(); // Option<String> — "jpg"
```

### Multiple File Uploads

```rust
let files = form.files("documents");  // &[UploadedFile]
for file in files {
    file.store(&app, "documents").await?;
}
```

### StoredFile Result

Every store operation returns a `StoredFile`:

```rust
pub struct StoredFile {
    pub disk: String,              // "local" or "s3"
    pub path: String,              // "avatars/uuid.jpg"
    pub name: String,              // "uuid.jpg"
    pub size: u64,                 // bytes
    pub content_type: Option<String>, // "image/jpeg"
    pub url: Option<String>,       // public URL if available
}
```

---

## Storage Manager

For direct file operations (not from uploads):

```rust
let storage = app.storage()?;

// Write bytes
storage.put("data/report.json", serde_json::to_vec(&report)?).await?;

// Read bytes
let bytes = storage.get("data/report.json").await?;

// Check existence
if storage.exists("data/report.json").await? {
    // file exists
}

// Delete
storage.delete("data/old-report.json").await?;

// Copy / Move
storage.copy("data/report.json", "backups/report.json").await?;
storage.move_to("temp/upload.csv", "data/import.csv").await?;

// Get URL
let url = storage.url("avatars/profile.jpg")?;

// Temporary URL (signed, for private S3 files)
let url = storage.temporary_url("documents/contract.pdf", DateTime::now().add_days(1)).await?;
```

### Working with Specific Disks

```rust
let storage = app.storage()?;

// Default disk (configured in [storage] default = "local")
let local = storage.default_disk()?;

// Named disk
let s3 = storage.disk("s3")?;
s3.put("exports/data.csv", csv_bytes).await?;
let url = s3.url("exports/data.csv")?;

// List configured disks
let disks = storage.configured_disks();  // ["local", "s3"]
```

---

## Image Processing

Chainable pipeline for transforming images. Works with files from disk or raw bytes.

### Opening Images

```rust
use forge::imaging::{ImageProcessor, ImageFormat, Rotation};

// From file path
let img = ImageProcessor::open("uploads/photo.jpg")?;

// From bytes (e.g., from storage)
let bytes = app.storage()?.get("avatars/profile.jpg").await?;
let img = ImageProcessor::from_bytes(&bytes)?;

// Check dimensions
println!("{}x{}", img.width(), img.height());
```

### Transformations

All methods return `Self` for chaining:

```rust
let result = ImageProcessor::open("photo.jpg")?
    .resize(800, 600)              // exact dimensions (stretches)
    .resize_to_fit(800, 600)       // fit within bounds (preserves aspect ratio)
    .resize_to_fill(800, 600)      // fill bounds (crops excess)
    .crop(10, 10, 200, 200)        // crop region (x, y, width, height)
    .rotate(Rotation::Deg90)       // rotate 90°, 180°, or 270°
    .flip_horizontal()             // mirror horizontally
    .flip_vertical()               // mirror vertically
    .grayscale()                   // convert to grayscale
    .blur(2.0)                     // Gaussian blur (sigma)
    .brightness(20)                // adjust brightness (-255 to +255)
    .contrast(1.5)                 // adjust contrast
    .quality(85)                   // JPEG/WebP quality (1-100)
    .to_bytes(ImageFormat::Jpeg)?; // output as bytes
```

### Saving

```rust
// Save to file (format inferred from extension)
img.save("output.jpg")?;

// Save with explicit format
img.save_as("output.webp", ImageFormat::WebP)?;

// Get bytes (for storing in storage)
let bytes = img.to_bytes(ImageFormat::Png)?;
app.storage()?.put("thumbnails/photo.png", bytes).await?;
```

### Supported Formats

| Format | Extension | Read | Write |
|--------|-----------|------|-------|
| JPEG | `.jpg`, `.jpeg` | Yes | Yes |
| PNG | `.png` | Yes | Yes |
| WebP | `.webp` | Yes | Yes |
| GIF | `.gif` | Yes | Yes |
| BMP | `.bmp` | Yes | Yes |
| TIFF | `.tiff`, `.tif` | Yes | Yes |
| AVIF | `.avif` | Yes | Yes |
| ICO | `.ico` | Yes | Yes |

---

## Upload → Process → Store

Common pattern: receive upload, process image, store result:

```rust
async fn upload_avatar(
    State(app): State<AppContext>,
    Auth(user): Auth<User>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse> {
    let form = MultipartForm::from_multipart(&mut multipart).await?;
    let file = form.file("avatar")?;

    // Process: resize to 256x256, optimize quality
    let processed = ImageProcessor::open(&file.temp_path)?
        .resize_to_fill(256, 256)
        .quality(80)
        .to_bytes(ImageFormat::WebP)?;

    // Store processed image
    let storage = app.storage()?;
    let path = format!("avatars/{}.webp", user.id);
    let stored = storage.put(&path, processed).await?;

    Ok(Json(json!({ "avatar_url": stored.url })))
}
```

### Generate Multiple Sizes

```rust
async fn upload_photo(app: &AppContext, file: &UploadedFile) -> Result<PhotoUrls> {
    let storage = app.storage()?;
    let img = ImageProcessor::open(&file.temp_path)?;
    let name = file.generate_storage_name();
    let stem = name.trim_end_matches(&format!(".{}", file.original_extension().unwrap_or_default()));

    // Original
    let original = file.store(app, "photos").await?;

    // Thumbnail (150x150)
    let thumb_bytes = ImageProcessor::open(&file.temp_path)?
        .resize_to_fill(150, 150)
        .quality(75)
        .to_bytes(ImageFormat::WebP)?;
    storage.put(&format!("photos/thumbs/{stem}.webp"), thumb_bytes).await?;

    // Medium (800px wide)
    let medium_bytes = ImageProcessor::open(&file.temp_path)?
        .resize_to_fit(800, 800)
        .quality(85)
        .to_bytes(ImageFormat::WebP)?;
    storage.put(&format!("photos/medium/{stem}.webp"), medium_bytes).await?;

    Ok(PhotoUrls {
        original: original.url.unwrap_or_default(),
        thumb: storage.url(&format!("photos/thumbs/{stem}.webp"))?,
        medium: storage.url(&format!("photos/medium/{stem}.webp"))?,
    })
}
```

---

## Custom Storage Drivers

Register via ServiceProvider or Plugin:

```rust
registrar.register_storage_driver("gcs", Arc::new(|config, table| {
    Box::pin(async move {
        let bucket = table.get("bucket").and_then(|v| v.as_str()).unwrap_or_default();
        Ok(Arc::new(GcsAdapter::new(bucket)) as Arc<dyn StorageAdapter>)
    })
}));
```

Then configure:

```toml
[storage.disks.gcs]
driver = "gcs"
bucket = "my-bucket"
```

Use identically to built-in drivers:

```rust
let gcs = app.storage()?.disk("gcs")?;
gcs.put("file.txt", b"hello").await?;
```
