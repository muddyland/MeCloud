//! Permissions for the app passwords MeCloud creates on Stalwart 1.0.3+.
//!
//! Stalwart 1.0.3 scopes OAuth tokens: MeCloud's token carries the
//! interoperable scopes (mail, contacts, calendars, files) plus
//! urn:stalwart:oauth:scope:security, and a scoped token may not create a
//! credential with more permissions than it holds. The default for an app
//! password ("Inherit": every permission of the account) is therefore refused,
//! and the password must list its permissions ("Replace"). This is the
//! permission set of the interoperable scopes, generated from Stalwart v1.0.3
//! crates/common/src/auth/oauth/scope.rs (the ALL, INTEROPERABLE, MAIL,
//! CONTACTS, CALENDARS, FILES and DAV groups). It leaves out the security and
//! settings permissions, so a device password cannot mint more passwords or
//! change the account.
//!
//! frontend/src/lib/stalwartPermissions.js holds the same list; a test keeps
//! the two in step.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

pub const INTEROPERABLE_PERMISSIONS: &[&str] = &[
    "authenticate",
    "authenticateWithAlias",
    "jmapPushSubscriptionGet",
    "jmapPushSubscriptionCreate",
    "jmapPushSubscriptionUpdate",
    "jmapPushSubscriptionDestroy",
    "jmapPrincipalGet",
    "jmapPrincipalQuery",
    "jmapPrincipalChanges",
    "jmapPrincipalQueryChanges",
    "jmapQuotaGet",
    "jmapQuotaChanges",
    "jmapQuotaQuery",
    "jmapQuotaQueryChanges",
    "jmapBlobGet",
    "jmapBlobCopy",
    "jmapBlobLookup",
    "jmapBlobUpload",
    "jmapShareNotificationGet",
    "jmapShareNotificationChanges",
    "jmapShareNotificationQuery",
    "jmapShareNotificationQueryChanges",
    "jmapShareNotificationCreate",
    "jmapShareNotificationUpdate",
    "jmapShareNotificationDestroy",
    "jmapCoreEcho",
    "jmapMetadataGet",
    "jmapMetadataSet",
    "jmapMetadataPrivate",
    "emailSend",
    "emailReceive",
    "jmapMailboxGet",
    "jmapMailboxChanges",
    "jmapMailboxQuery",
    "jmapMailboxQueryChanges",
    "jmapMailboxCreate",
    "jmapMailboxUpdate",
    "jmapMailboxDestroy",
    "jmapThreadGet",
    "jmapThreadChanges",
    "jmapEmailGet",
    "jmapEmailChanges",
    "jmapEmailQuery",
    "jmapEmailQueryChanges",
    "jmapEmailCreate",
    "jmapEmailUpdate",
    "jmapEmailDestroy",
    "jmapEmailCopy",
    "jmapEmailImport",
    "jmapEmailParse",
    "jmapSearchSnippetGet",
    "jmapIdentityGet",
    "jmapIdentityChanges",
    "jmapIdentityCreate",
    "jmapIdentityUpdate",
    "jmapIdentityDestroy",
    "jmapEmailSubmissionGet",
    "jmapEmailSubmissionChanges",
    "jmapEmailSubmissionQuery",
    "jmapEmailSubmissionQueryChanges",
    "jmapEmailSubmissionCreate",
    "jmapEmailSubmissionUpdate",
    "jmapEmailSubmissionDestroy",
    "jmapVacationResponseGet",
    "jmapVacationResponseCreate",
    "jmapVacationResponseUpdate",
    "jmapVacationResponseDestroy",
    "jmapSieveScriptGet",
    "jmapSieveScriptQuery",
    "jmapSieveScriptValidate",
    "jmapSieveScriptCreate",
    "jmapSieveScriptUpdate",
    "jmapSieveScriptDestroy",
    "imapAuthenticate",
    "imapAclGet",
    "imapAclSet",
    "imapMyRights",
    "imapListRights",
    "imapAppend",
    "imapCapability",
    "imapId",
    "imapCopy",
    "imapMove",
    "imapCreate",
    "imapDelete",
    "imapEnable",
    "imapExpunge",
    "imapFetch",
    "imapIdle",
    "imapList",
    "imapLsub",
    "imapNamespace",
    "imapRename",
    "imapSearch",
    "imapSort",
    "imapSelect",
    "imapExamine",
    "imapStatus",
    "imapStore",
    "imapSubscribe",
    "imapThread",
    "imapMetadataGet",
    "imapMetadataSet",
    "imapMetadataPrivate",
    "pop3Authenticate",
    "pop3List",
    "pop3Uidl",
    "pop3Stat",
    "pop3Retr",
    "pop3Dele",
    "sieveAuthenticate",
    "sieveListScripts",
    "sieveSetActive",
    "sieveGetScript",
    "sievePutScript",
    "sieveDeleteScript",
    "sieveRenameScript",
    "sieveCheckScript",
    "sieveHaveSpace",
    "sysSpamTrainingSampleGet",
    "sysSpamTrainingSampleCreate",
    "sysSpamTrainingSampleDestroy",
    "sysSpamTrainingSampleQuery",
    "jmapAddressBookGet",
    "jmapAddressBookChanges",
    "jmapAddressBookCreate",
    "jmapAddressBookUpdate",
    "jmapAddressBookDestroy",
    "jmapContactCardGet",
    "jmapContactCardChanges",
    "jmapContactCardQuery",
    "jmapContactCardQueryChanges",
    "jmapContactCardCreate",
    "jmapContactCardUpdate",
    "jmapContactCardDestroy",
    "jmapContactCardCopy",
    "jmapContactCardParse",
    "davCardPropFind",
    "davCardPropPatch",
    "davCardGet",
    "davCardMkCol",
    "davCardDelete",
    "davCardPut",
    "davCardCopy",
    "davCardMove",
    "davCardLock",
    "davCardAcl",
    "davCardQuery",
    "davCardMultiGet",
    "calendarAlarmsSend",
    "calendarSchedulingSend",
    "calendarSchedulingReceive",
    "jmapPrincipalGetAvailability",
    "jmapCalendarGet",
    "jmapCalendarChanges",
    "jmapCalendarCreate",
    "jmapCalendarUpdate",
    "jmapCalendarDestroy",
    "jmapCalendarEventGet",
    "jmapCalendarEventChanges",
    "jmapCalendarEventQuery",
    "jmapCalendarEventQueryChanges",
    "jmapCalendarEventCreate",
    "jmapCalendarEventUpdate",
    "jmapCalendarEventDestroy",
    "jmapCalendarEventCopy",
    "jmapCalendarEventParse",
    "jmapCalendarEventNotificationGet",
    "jmapCalendarEventNotificationChanges",
    "jmapCalendarEventNotificationQuery",
    "jmapCalendarEventNotificationQueryChanges",
    "jmapCalendarEventNotificationCreate",
    "jmapCalendarEventNotificationUpdate",
    "jmapCalendarEventNotificationDestroy",
    "jmapParticipantIdentityGet",
    "jmapParticipantIdentityChanges",
    "jmapParticipantIdentityCreate",
    "jmapParticipantIdentityUpdate",
    "jmapParticipantIdentityDestroy",
    "davCalPropFind",
    "davCalPropPatch",
    "davCalGet",
    "davCalMkCol",
    "davCalDelete",
    "davCalPut",
    "davCalCopy",
    "davCalMove",
    "davCalLock",
    "davCalAcl",
    "davCalQuery",
    "davCalMultiGet",
    "davCalFreeBusyQuery",
    "jmapFileNodeGet",
    "jmapFileNodeChanges",
    "jmapFileNodeQuery",
    "jmapFileNodeQueryChanges",
    "jmapFileNodeCreate",
    "jmapFileNodeUpdate",
    "jmapFileNodeDestroy",
    "jmapFileNodeCopy",
    "davFilePropFind",
    "davFilePropPatch",
    "davFileGet",
    "davFileMkCol",
    "davFileDelete",
    "davFilePut",
    "davFileCopy",
    "davFileMove",
    "davFileLock",
    "davFileAcl",
    "davSyncCollection",
    "davExpandProperty",
    "davPrincipalAcl",
    "davPrincipalList",
    "davPrincipalMatch",
    "davPrincipalSearch",
    "davPrincipalSearchPropSet",
];

pub fn is_forbidden(error: Option<&Value>) -> bool {
    error.and_then(|e| e.get("type")).and_then(Value::as_str) == Some("forbidden")
}

/// The permissions a `forbidden` SetError names as not grantable (may be
/// empty), from "…not authorized to grant permissions: a, b."
pub fn refused_permissions(error: Option<&Value>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if !is_forbidden(error) {
        return out;
    }
    let description = error
        .and_then(|e| e.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    const NEEDLE: &str = "not authorized to grant permissions:";
    // ASCII lower-casing keeps byte offsets, so the index is valid in both.
    let Some(at) = description.to_ascii_lowercase().find(NEEDLE) else {
        return out;
    };
    let list = description[at + NEEDLE.len()..].trim();
    let list = list.trim_end_matches('.');
    for name in list.split(|c: char| c.is_whitespace() || c == ',') {
        if !name.is_empty() {
            out.insert(name.to_string());
        }
    }
    out
}

/// An x:CredentialPermissions value granting the interoperable set, minus
/// `exclude`.
pub fn replace_permissions(exclude: &BTreeSet<String>) -> Value {
    let mut permissions = Map::new();
    for p in INTEROPERABLE_PERMISSIONS {
        if !exclude.contains(*p) {
            permissions.insert(p.to_string(), Value::Bool(true));
        }
    }
    json!({ "@type": "Replace", "permissions": permissions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_password_cannot_manage_the_account() {
        for p in [
            "sysAppPasswordCreate",
            "sysAccountPasswordUpdate",
            "sysAccountSettingsUpdate",
            "sysApiKeyCreate",
        ] {
            assert!(!INTEROPERABLE_PERMISSIONS.contains(&p), "{p}");
        }
    }

    #[test]
    fn the_frontend_list_matches() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../frontend/src/lib/stalwartPermissions.js"
        );
        let Ok(js) = std::fs::read_to_string(path) else {
            eprintln!("frontend sources not present; skipped");
            return;
        };
        let block = js
            .split_once("Object.freeze([")
            .unwrap()
            .1
            .split_once("]);")
            .unwrap()
            .0;
        let names: Vec<&str> = block
            .split('\'')
            .skip(1)
            .step_by(2)
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
            .collect();
        assert_eq!(names, INTEROPERABLE_PERMISSIONS);
    }

    #[test]
    fn refusals_are_read() {
        let refusal = json!({"type": "forbidden",
            "description": "You are not authorized to grant permissions: interactAi, sysAccountSettingsUpdate."});
        let names: Vec<String> = refused_permissions(Some(&refusal)).into_iter().collect();
        assert_eq!(names, ["interactAi", "sysAccountSettingsUpdate"]);
        assert!(refused_permissions(Some(&json!({"type": "forbidden"}))).is_empty());
        assert!(refused_permissions(Some(&json!({"type": "invalidProperties", "description": "not authorized to grant permissions: x"}))).is_empty());
        assert!(refused_permissions(None).is_empty());
    }
}
