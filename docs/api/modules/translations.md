# translations

Model field translations across locales (HasTranslations)

[Back to index](../index.md)

## forge::translations

```rust
struct ModelTranslation
struct TranslatedFields
  fn from_entries( entries: Vec<(String, String)>, current_locale: &str, default_locale: &str, ) -> Self
  fn get(&self, locale: &str) -> Option<&str>
trait HasTranslations
  fn translatable_type() -> &'static str
  fn translatable_id(&self) -> String
  fn set_translation<'life0, 'life1, 'life2, 'life3, 'life4, 'async_trait>(
  fn set_translations<'life0, 'life1, 'life2, 'life3, 'life4, 'life5, 'async_trait>(
  fn translation<'life0, 'life1, 'life2, 'life3, 'async_trait>(
  fn translations_for<'life0, 'life1, 'life2, 'async_trait>(
  fn translated_field<'life0, 'life1, 'life2, 'async_trait>(
  fn all_translations<'life0, 'life1, 'async_trait>(
  fn delete_translations<'life0, 'life1, 'life2, 'async_trait>(
fn current_locale(app: &AppContext) -> String
```

