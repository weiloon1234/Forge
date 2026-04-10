use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::foundation::{Error, Result};

type SharedService = Arc<dyn Any + Send + Sync>;
type ServiceFactory = Arc<dyn Fn(&Container) -> Result<SharedService> + Send + Sync>;

#[derive(Clone)]
enum ServiceEntry {
    Singleton(SharedService),
    Factory(ServiceFactory),
}

#[derive(Clone, Default)]
pub struct Container {
    entries: Arc<RwLock<HashMap<TypeId, ServiceEntry>>>,
}

impl Container {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn singleton<T>(&self, value: T) -> Result<()>
    where
        T: Send + Sync + 'static,
    {
        self.singleton_arc(Arc::new(value))
    }

    pub fn singleton_arc<T>(&self, value: Arc<T>) -> Result<()>
    where
        T: Send + Sync + 'static,
    {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| Error::message("container lock poisoned"))?;
        let type_id = TypeId::of::<T>();
        if entries.contains_key(&type_id) {
            return Err(Error::message(format!(
                "service `{}` already registered",
                std::any::type_name::<T>()
            )));
        }

        let shared: SharedService = value;
        entries.insert(type_id, ServiceEntry::Singleton(shared));
        Ok(())
    }

    pub fn factory<T, F>(&self, factory: F) -> Result<()>
    where
        T: Send + Sync + 'static,
        F: Fn(&Container) -> Result<T> + Send + Sync + 'static,
    {
        self.factory_arc(move |container| {
            let value = factory(container)?;
            Ok(Arc::new(value))
        })
    }

    pub fn factory_arc<T, F>(&self, factory: F) -> Result<()>
    where
        T: Send + Sync + 'static,
        F: Fn(&Container) -> Result<Arc<T>> + Send + Sync + 'static,
    {
        let mut entries = self
            .entries
            .write()
            .map_err(|_| Error::message("container lock poisoned"))?;
        let type_id = TypeId::of::<T>();
        if entries.contains_key(&type_id) {
            return Err(Error::message(format!(
                "service `{}` already registered",
                std::any::type_name::<T>()
            )));
        }

        let wrapped: ServiceFactory = Arc::new(move |container| {
            let service: Arc<T> = factory(container)?;
            let shared: SharedService = service;
            Ok(shared)
        });
        entries.insert(type_id, ServiceEntry::Factory(wrapped));
        Ok(())
    }

    pub fn resolve<T>(&self) -> Result<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        let entry = {
            let entries = self
                .entries
                .read()
                .map_err(|_| Error::message("container lock poisoned"))?;
            entries.get(&TypeId::of::<T>()).cloned()
        }
        .ok_or_else(|| {
            Error::message(format!(
                "service `{}` not registered",
                std::any::type_name::<T>()
            ))
        })?;

        let shared = match entry {
            ServiceEntry::Singleton(value) => value,
            ServiceEntry::Factory(factory) => factory(self)?,
        };

        Arc::downcast::<T>(shared).map_err(|_| {
            Error::message(format!(
                "service `{}` registered with mismatched type",
                std::any::type_name::<T>()
            ))
        })
    }

    pub fn contains<T>(&self) -> bool
    where
        T: Send + Sync + 'static,
    {
        self.entries
            .read()
            .ok()
            .and_then(|entries| entries.get(&TypeId::of::<T>()).cloned())
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::Container;

    #[test]
    fn resolves_singletons_and_factories() {
        let container = Container::new();
        container.singleton::<String>("forge".to_string()).unwrap();
        container
            .factory::<usize, _>(|inner| Ok(inner.resolve::<String>()?.len()))
            .unwrap();

        assert_eq!(container.resolve::<String>().unwrap().as_str(), "forge");
        assert_eq!(*container.resolve::<usize>().unwrap(), 5);
    }

    #[test]
    fn rejects_duplicate_registrations() {
        let container = Container::new();
        container.singleton::<String>("forge".to_string()).unwrap();

        let error = container
            .singleton::<String>("duplicate".to_string())
            .unwrap_err();
        assert!(error.to_string().contains("already registered"));
    }
}
