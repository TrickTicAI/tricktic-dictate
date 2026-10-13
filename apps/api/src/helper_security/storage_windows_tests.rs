//! Exercise the real Windows ACL evaluator with owner and non-owner identities;
//! no accounts, passwords or machine permissions are created/changed.
use super::*;
#[repr(C)]
struct Request {
    desired: u32,
    principal: Handle,
    objects: Handle,
    count: u32,
    arguments: Handle,
}
#[repr(C)]
struct Reply {
    count: u32,
    granted: *mut u32,
    sacl: *mut u32,
    error: *mut u32,
}
#[repr(C)]
struct Luid {
    low: u32,
    high: i32,
}
#[link(name = "authz")]
extern "system" {
    fn AuthzInitializeResourceManager(
        flags: u32,
        access: Handle,
        compute: Handle,
        free: Handle,
        name: *const u16,
        manager: *mut Handle,
    ) -> i32;
    fn AuthzInitializeContextFromSid(
        flags: u32,
        sid: Handle,
        manager: Handle,
        expiration: Handle,
        id: Luid,
        arguments: Handle,
        context: *mut Handle,
    ) -> i32;
    fn AuthzAccessCheck(
        flags: u32,
        context: Handle,
        request: *const Request,
        audit: Handle,
        descriptor: Handle,
        optional: Handle,
        count: u32,
        reply: *mut Reply,
        results: Handle,
    ) -> i32;
    fn AuthzFreeContext(context: Handle) -> i32;
    fn AuthzFreeResourceManager(manager: Handle) -> i32;
}
#[link(name = "advapi32")]
extern "system" {
    fn ConvertStringSidToSidW(text: *const u16, sid: *mut Handle) -> i32;
}

#[test]
fn private_acl_grants_owner_and_denies_other_principals() {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("writer-acl-{}", uuid::Uuid::new_v4()));
    protect_private_dir(&root).unwrap();
    std::fs::write(root.join("synthetic-audio"), b"synthetic").unwrap();
    unsafe {
        let null = std::ptr::null_mut();
        let mut manager = null;
        assert_ne!(
            AuthzInitializeResourceManager(1, null, null, null, std::ptr::null(), &mut manager),
            0
        );
        for path in [&root, &root.join("synthetic-audio")] {
            let path: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = CreateFileW(path.as_ptr(), 0x20000, 1 | 2, null, 3, 0x02000000, null);
            assert_ne!(handle as isize, -1);
            let handle = Owned(handle);
            let (mut owner, mut descriptor) = (null, null);
            assert_eq!(
                GetSecurityInfo(
                    handle.0,
                    1,
                    1 | 2 | 4,
                    &mut owner,
                    null,
                    null,
                    null,
                    &mut descriptor
                ),
                0
            );
            let descriptor = Descriptor(descriptor);
            let mut identities = vec![(owner, true)];
            let mut allocated = Vec::new();
            // Everyone, Builtin Users and a different ordinary user SID.
            for sid in ["S-1-1-0", "S-1-5-32-545", "S-1-5-21-111-222-333-1001"] {
                let text: Vec<_> = sid.encode_utf16().chain(Some(0)).collect();
                let mut sid = null;
                assert_ne!(ConvertStringSidToSidW(text.as_ptr(), &mut sid), 0);
                allocated.push(Descriptor(sid));
                identities.push((sid, false));
            }
            for (sid, allowed) in identities {
                let mut context = null;
                assert_ne!(
                    AuthzInitializeContextFromSid(
                        2,
                        sid,
                        manager,
                        null,
                        Luid { low: 0, high: 0 },
                        null,
                        &mut context
                    ),
                    0
                );
                let request = Request {
                    desired: 1 | 2,
                    principal: null,
                    objects: null,
                    count: 0,
                    arguments: null,
                };
                let (mut granted, mut error, mut sacl) = (0, 0, 0);
                let mut reply = Reply {
                    count: 1,
                    granted: &mut granted,
                    error: &mut error,
                    sacl: &mut sacl,
                };
                assert_ne!(
                    AuthzAccessCheck(
                        0,
                        context,
                        &request,
                        null,
                        descriptor.0,
                        null,
                        0,
                        &mut reply,
                        null
                    ),
                    0
                );
                AuthzFreeContext(context);
                assert_eq!(error == 0 && granted & 3 == 3, allowed);
            }
        }
        AuthzFreeResourceManager(manager);
    }
    std::fs::remove_dir_all(root).unwrap();
}
