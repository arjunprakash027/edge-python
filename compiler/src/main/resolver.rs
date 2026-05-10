use crate::modules::packages::{NativeBinding, Resolved, Resolver, parse_manifest, walk_up_dirs, dir_of, join_relative};
use crate::util::fx::FxHashSet;
use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use crate::s;

use super::{ModuleEntry, host_fetch_bytes, manifests, registry};
use super::abi_bridge::make_native_binding;

/* Hard cap on packages.json `extends` chain length. Prevents an attacker-
   crafted manifest from looping the resolver indefinitely; 32 is well above
   any sane real-world workspace depth. */
const MAX_PACKAGES_HOPS: u32 = 32;

pub(super) struct WasmHostResolver { pub(super) dir: String }

impl Resolver for WasmHostResolver {
    fn resolve(&mut self, spec: &str) -> Result<Resolved, String> {
        if !spec.contains('/') {
            let dir = self.dir.clone();
            return self.resolve_bare(spec, &dir);
        }
        let canonical = if spec.contains("://") || spec.starts_with('/') {
            spec.to_string()
        } else {
            join_relative(&self.dir, spec)
        };
        self.resolve_canonical(&canonical)
    }

    fn fetch_bytes(&mut self,spec: &str,expected_hash: Option<[u8; 32]>) -> Result<Vec<u8>, String> {
        let mut len: u32 = 0;
        let hash_ptr = expected_hash.as_ref().map(|h| h.as_ptr()).unwrap_or(core::ptr::null());
        let ptr = unsafe {
            host_fetch_bytes(spec.as_ptr(), spec.len() as u32, hash_ptr, &mut len as *mut u32)
        };
        if ptr.is_null() {
            return Err(s!("no bytes cached by host for '", str spec, "'"));
        }
        Ok(unsafe { Vec::from_raw_parts(ptr, len as usize, len as usize) })
    }

    fn child(&self, spec: &str) -> Box<dyn Resolver> {
        Box::new(WasmHostResolver { dir: dir_of(spec).to_string() })
    }
}

impl WasmHostResolver {
    fn resolve_bare(&mut self, name: &str, start_dir: &str) -> Result<Resolved, String> {
        let mut visited: FxHashSet<String> = FxHashSet::default();
        let mut search_dir = start_dir.to_string();
        let mut hops: u32 = 0;
        loop {
            if hops > MAX_PACKAGES_HOPS {
                return Err(s!(
                    "packages.json walk-up exceeded ",
                    int MAX_PACKAGES_HOPS as i64,
                    " hops resolving '", str name, "'"));
            }
            hops += 1;

            let mut hit: Option<(String, Option<String>, Option<String>)> = None;
            for dir in walk_up_dirs(&search_dir) {
                let m_spec = s!(str &dir, "packages.json");
                if let Some((target, ext)) = self.lookup_in_manifest(&m_spec, name)? {
                    hit = Some((dir, target, ext));
                    break;
                }
            }
            let Some((dir, target, ext)) = hit else {
                return Err(s!("no packages.json above '", str start_dir, "' declares '", str name, "'"));
            };
            if let Some(target) = target {
                let canonical = join_relative(&dir, &target);
                return self.resolve_canonical(&canonical);
            }
            let m_spec = s!(str &dir, "packages.json");
            if let Some(ext) = ext {
                if !visited.insert(m_spec) {
                    return Err(s!("circular extends chain in packages.json"));
                }
                let mut next = join_relative(&dir, &ext);
                if !next.ends_with('/') { next.push('/'); }
                search_dir = next;
                continue;
            }
            return Err(s!(
                "alias '", str name, "' not declared in '", str &m_spec, "'\n",
                "help: declare it, add \"extends\": \"..\" to inherit, or use a quoted path",
            ));
        }
    }

    #[allow(clippy::type_complexity)]
    fn lookup_in_manifest(&mut self, m_spec: &str, name: &str) -> Result<Option<(Option<String>, Option<String>)>, String> {
        let cache = unsafe { manifests() };
        if let Some((_, m)) = cache.iter().find(|(s, _)| s == m_spec) {
            return Ok(Some((m.imports.get(name).cloned(), m.extends.clone())));
        }
        // Walk-up fetch — manifests aren't pinned by URL fragment, so no hash.
        let bytes = match self.fetch_bytes(m_spec, None) {
            Ok(b) => b,
            Err(_) => return Ok(None),
        };
        let parsed = parse_manifest(&bytes).map_err(|e| s!("packages.json at '", str m_spec, "': ", str &e))?;
        let target = parsed.imports.get(name).cloned();
        let ext = parsed.extends.clone();
        cache.push((m_spec.to_string(), parsed));
        Ok(Some((target, ext)))
    }

    fn resolve_canonical(&self, spec: &str) -> Result<Resolved, String> {
        let reg = unsafe { registry() };
        let entry = reg.iter().find(|(s, _)| s == spec).ok_or_else(|| s!("module '", str spec, "' not registered (host did not pre-fetch / register before run())"))?;
        match &entry.1 {
            ModuleEntry::Code(src) => Ok(Resolved::Code {
                src: src.clone(),
                canonical: spec.to_string(),
            }),
            ModuleEntry::Native(funcs) => {
                let bindings: Vec<NativeBinding> = funcs.iter()
                    .map(|(name, id)| make_native_binding(name.clone(), *id))
                    .collect();
                Ok(Resolved::Native {
                    bindings,
                    canonical: spec.to_string(),
                })
            }
        }
    }
}
