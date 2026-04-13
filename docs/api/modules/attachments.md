# attachments

File attachments with lifecycle (HasAttachments)

[Back to index](../index.md)

## forge::attachments

```rust
struct Attachment
  fn upload(file: UploadedFile) -> AttachmentUploadBuilder
  fn is_image(&self) -> bool
  fn is_video(&self) -> bool
  fn is_audio(&self) -> bool
  fn is_document(&self) -> bool
  fn extension(&self) -> Option<&str>
  fn human_size(&self) -> String
  async fn url(&self, app: &AppContext) -> Result<String>
  async fn temporary_url( &self, app: &AppContext, expires_at: DateTime, ) -> Result<String>
  async fn image(&self, app: &AppContext) -> Result<ImageProcessor>
struct AttachmentUploadBuilder
  fn collection(self, collection: impl Into<String>) -> Self
  fn disk(self, disk: impl Into<String>) -> Self
  fn resize(self, width: u32, height: u32) -> Self
  fn resize_to_fit(self, max_width: u32, max_height: u32) -> Self
  fn resize_to_fill(self, width: u32, height: u32) -> Self
  fn quality(self, quality: u8) -> Self
  async fn store( self, app: &AppContext, attachable_type: &str, attachable_id: &str, ) -> Result<Attachment>
trait HasAttachments
  fn attachable_type() -> &'static str
  fn attachable_id(&self) -> String
  fn attach<'life0, 'life1, 'life2, 'async_trait>(
  fn attachment<'life0, 'life1, 'life2, 'async_trait>(
  fn attachments<'life0, 'life1, 'life2, 'async_trait>(
  fn detach<'life0, 'life1, 'life2, 'async_trait>(
  fn detach_keep_file<'life0, 'life1, 'life2, 'async_trait>(
  fn detach_all<'life0, 'life1, 'life2, 'async_trait>(
```

