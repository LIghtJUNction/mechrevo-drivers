/*
 * Native x64 MSI maintenance guard. This DLL never opens an OEM device.
 *
 * Build with an x64 MSVC developer prompt, /LD /MT /TC, and these quoted
 * narrow-string definitions (there are deliberately no development defaults):
 *   MECHREVO_WINDOWS_SOURCE_COMMIT = 40 lowercase hexadecimal characters
 *   MECHREVO_WINDOWS_RELEASE_ID = 1..128 characters from [A-Za-z0-9._-]
 * Required libraries: msi, advapi32, bcrypt, shell32, ole32, uuid.
 *
 * PrepareMaintenance is immediate, after target-path costing. The deferred
 * AcquireMaintenance and rollback/commit ReleaseMaintenance actions must be
 * NoImpersonate. Schedule rollback before Acquire and commit last, before
 * InstallFinalize; reject RollbackDisabled. VerifyMaintenance is immediate
 * after InstallExecute (and after nested removal), before InstallFinalize;
 * unlike deferred actions, it can inspect actual session reboot flags.
 * RemoveExistingProducts must be
 * after InstallExecute and before the outer commit. Do not enable Restart
 * Manager writer termination. CustomActionData is nonce|ProductCode|mode.
 *
 * The mutex is held ONLY within one action. The persistent marker excludes
 * writers between actions/processes. A crash does not expire the marker.
 * Repair may replace a valid marker belonging to the SAME ProductCode only;
 * this relies on Windows Installer's serialized execute transactions, never
 * on a PID, clock, or guessed liveness. Nested upgrade uninstall only borrows
 * an existing valid marker and never removes it. Failed/partial marker writes
 * deliberately remain fail-closed for explicit administrator inspection.
 */

#ifndef _WIN32_WINNT
#define _WIN32_WINNT 0x0A00
#endif
#define WIN32_LEAN_AND_MEAN
#define COBJMACROS
#include <windows.h>
#include <msi.h>
#include <msiquery.h>
#include <aclapi.h>
#include <sddl.h>
#include <bcrypt.h>
#include <shlobj.h>
#include <knownfolders.h>
#include <strsafe.h>
#include <string.h>
#include <wchar.h>

#if !defined(_WIN64) || !defined(_M_X64)
#error This custom action must be compiled as native x64.
#endif
#ifndef MECHREVO_WINDOWS_SOURCE_COMMIT
#error MECHREVO_WINDOWS_SOURCE_COMMIT must be a quoted narrow string.
#endif
#ifndef MECHREVO_WINDOWS_RELEASE_ID
#error MECHREVO_WINDOWS_RELEASE_ID must be a quoted narrow string.
#endif

#pragma comment(lib, "msi.lib")
#pragma comment(lib, "advapi32.lib")
#pragma comment(lib, "bcrypt.lib")
#pragma comment(lib, "shell32.lib")
#pragma comment(lib, "ole32.lib")
#pragma comment(lib, "uuid.lib")

#define ARRAY_COUNT(a) (sizeof(a) / sizeof((a)[0]))
#define PATH_CAPACITY 32768u
#define MAX_HELD_DIRECTORIES 128u
#define MAX_GATE_BYTES 512u
#define MUTEX_ACCESS ((DWORD)0x00120001)
#define DIRECTORY_WRITE_ACCESS ((DWORD)(FILE_WRITE_DATA | FILE_APPEND_DATA | \
    FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES | FILE_DELETE_CHILD | DELETE | \
    WRITE_DAC | WRITE_OWNER | GENERIC_ALL | GENERIC_WRITE))
#define ANCESTOR_REPLACEMENT_ACCESS ((DWORD)(FILE_DELETE_CHILD | DELETE | \
    WRITE_DAC | WRITE_OWNER | GENERIC_ALL | GENERIC_WRITE))

static const WCHAR kMutexName[] = L"Global\\MechrevoConsole.Oem.v1.FnLock";
static const WCHAR kMutexSddl[] =
    L"O:BAG:BAD:P(A;;0x00120001;;;SY)(A;;0x00120001;;;BA)";
static const WCHAR kDirectorySddl[] =
    L"O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
static const WCHAR kFileSddl[] =
    L"O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)";
static const WCHAR kTrustedInstallerSid[] =
    L"S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
static const char kSourceCommit[] = MECHREVO_WINDOWS_SOURCE_COMMIT;
static const char kReleaseId[] = MECHREVO_WINDOWS_RELEASE_ID;

typedef struct Transaction {
    char nonce[65];
    char product[39];
    char mode[8];
} Transaction;

typedef struct Marker {
    Transaction transaction;
    char source[41];
    char release[129];
} Marker;

typedef struct HeldDirectories {
    HANDLE handles[MAX_HELD_DIRECTORIES];
    size_t count;
    WCHAR *path;
} HeldDirectories;

typedef enum DirectoryPolicy {
    DirectoryAncestor,
    DirectoryInstall,
    DirectoryPrivate
} DirectoryPolicy;

static UINT fail(MSIHANDLE installer, const WCHAR *stage, DWORD error)
{
    WCHAR message[512];
    MSIHANDLE record;
    if (error == ERROR_SUCCESS) {
        error = ERROR_INVALID_DATA;
    }
    if (FAILED(StringCchPrintfW(message, ARRAY_COUNT(message),
            L"Mechrevo maintenance guard: %s failed (Windows error %lu). "
            L"No hardware state was changed. Do not remove an unexplained "
            L"maintenance.json; inspect it as administrator.", stage, error))) {
        return ERROR_INSTALL_FAILURE;
    }
    record = MsiCreateRecord(0);
    if (record != 0) {
        MsiRecordSetStringW(record, 0, message);
        MsiProcessMessage(installer, INSTALLMESSAGE_ERROR, record);
        MsiCloseHandle(record);
    }
    return ERROR_INSTALL_FAILURE;
}

static void note(MSIHANDLE installer, const WCHAR *message)
{
    MSIHANDLE record = MsiCreateRecord(0);
    if (record != 0) {
        MsiRecordSetStringW(record, 0, message);
        MsiProcessMessage(installer, INSTALLMESSAGE_INFO, record);
        MsiCloseHandle(record);
    }
}

static BOOL lowercase_hex(const char *value, size_t length)
{
    size_t i;
    if (strlen(value) != length) {
        return FALSE;
    }
    for (i = 0; i < length; ++i) {
        if (!((value[i] >= '0' && value[i] <= '9') ||
                (value[i] >= 'a' && value[i] <= 'f'))) {
            return FALSE;
        }
    }
    return TRUE;
}

static BOOL product_code(const char *value)
{
    size_t i;
    if (strlen(value) != 38 || value[0] != '{' || value[37] != '}') {
        return FALSE;
    }
    for (i = 1; i < 37; ++i) {
        char c = value[i];
        if (i == 9 || i == 14 || i == 19 || i == 24) {
            if (c != '-') {
                return FALSE;
            }
        } else if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') ||
                (c >= 'A' && c <= 'F'))) {
            return FALSE;
        }
    }
    return TRUE;
}

static BOOL release_id(const char *value)
{
    size_t i;
    size_t length = strlen(value);
    if (length == 0 || length > 128) {
        return FALSE;
    }
    for (i = 0; i < length; ++i) {
        char c = value[i];
        if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                (c >= '0' && c <= '9') || c == '.' || c == '_' || c == '-')) {
            return FALSE;
        }
    }
    return TRUE;
}

static BOOL ordinary_mode(const char *value)
{
    return strcmp(value, "install") == 0 || strcmp(value, "remove") == 0 ||
        strcmp(value, "repair") == 0 || strcmp(value, "upgrade") == 0;
}

static BOOL read_property(MSIHANDLE installer, const WCHAR *name,
        WCHAR *value, DWORD capacity)
{
    DWORD length = capacity - 1;
    UINT result;
    value[0] = L'\0';
    result = MsiGetPropertyW(installer, name, value, &length);
    if (result != ERROR_SUCCESS || length >= capacity) {
        SetLastError(result == ERROR_SUCCESS ? ERROR_INVALID_DATA : result);
        return FALSE;
    }
    return TRUE;
}

static BOOL narrow_ascii(const WCHAR *input, char *output, size_t capacity)
{
    size_t i;
    for (i = 0; input[i] != L'\0'; ++i) {
        if (i + 1 >= capacity || input[i] < 0x20 || input[i] > 0x7e) {
            SetLastError(ERROR_INVALID_DATA);
            return FALSE;
        }
        output[i] = (char)input[i];
    }
    output[i] = '\0';
    return TRUE;
}

static BOOL transaction_data(MSIHANDLE installer, Transaction *transaction)
{
    WCHAR property[128];
    char data[128];
    char *first;
    char *second;
    if (!read_property(installer, L"CustomActionData", property,
            (DWORD)ARRAY_COUNT(property)) ||
            !narrow_ascii(property, data, sizeof(data))) {
        return FALSE;
    }
    first = strchr(data, '|');
    if (first == NULL) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    *first++ = '\0';
    second = strchr(first, '|');
    if (second == NULL) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    *second++ = '\0';
    if (!lowercase_hex(data, 64) || !product_code(first) ||
            (!ordinary_mode(second) && strcmp(second, "nested") != 0)) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    memcpy(transaction->nonce, data, 65);
    memcpy(transaction->product, first, 39);
    strcpy_s(transaction->mode, sizeof(transaction->mode), second);
    return TRUE;
}

static BOOL trusted_sid(PSID sid, BOOL allow_trusted_installer)
{
    PSID trusted_installer = NULL;
    BOOL result;
    if (sid == NULL || !IsValidSid(sid)) {
        return FALSE;
    }
    if (IsWellKnownSid(sid, WinLocalSystemSid) ||
            IsWellKnownSid(sid, WinBuiltinAdministratorsSid)) {
        return TRUE;
    }
    if (!allow_trusted_installer ||
            !ConvertStringSidToSidW(kTrustedInstallerSid, &trusted_installer)) {
        return FALSE;
    }
    result = EqualSid(sid, trusted_installer);
    LocalFree(trusted_installer);
    return result;
}

static BOOL allowed_ace_sid(ACCESS_ALLOWED_ACE *ace, PSID *sid)
{
    DWORD prefix = (DWORD)FIELD_OFFSET(ACCESS_ALLOWED_ACE, SidStart);
    if ((DWORD)ace->Header.AceSize < prefix + GetSidLengthRequired(0)) {
        return FALSE;
    }
    *sid = (PSID)&ace->SidStart;
    return IsValidSid(*sid) && (DWORD)ace->Header.AceSize ==
        prefix + GetLengthSid(*sid);
}

/* Validate the descriptor of the opened object, never a second pathname. */
static BOOL exact_security(HANDLE handle, SE_OBJECT_TYPE type,
        DWORD mask, BYTE flags)
{
    PSECURITY_DESCRIPTOR descriptor = NULL;
    PSID owner = NULL;
    PSID group = NULL;
    PACL acl = NULL;
    SECURITY_DESCRIPTOR_CONTROL control = 0;
    DWORD revision = 0;
    DWORD result;
    DWORD i;
    BOOL found_system = FALSE;
    BOOL found_admins = FALSE;
    BOOL valid = FALSE;
    result = GetSecurityInfo(handle, type,
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION |
        DACL_SECURITY_INFORMATION, &owner, &group, &acl, NULL, &descriptor);
    if (result != ERROR_SUCCESS) {
        SetLastError(result);
        return FALSE;
    }
    if (!trusted_sid(owner, FALSE) || !trusted_sid(group, FALSE) || acl == NULL ||
            !IsValidAcl(acl) || acl->AceCount != 2 ||
            !GetSecurityDescriptorControl(descriptor, &control, &revision) ||
            (control & SE_DACL_PROTECTED) == 0 ||
            (control & (SE_OWNER_DEFAULTED | SE_GROUP_DEFAULTED |
                SE_DACL_DEFAULTED)) != 0) {
        goto done;
    }
    for (i = 0; i < 2; ++i) {
        ACCESS_ALLOWED_ACE *ace = NULL;
        PSID sid;
        if (!GetAce(acl, i, (void **)&ace) ||
                ace->Header.AceType != ACCESS_ALLOWED_ACE_TYPE ||
                ace->Header.AceSize < sizeof(ACCESS_ALLOWED_ACE) ||
                ace->Header.AceFlags != flags || ace->Mask != mask ||
                !allowed_ace_sid(ace, &sid)) {
            goto done;
        }
        if (IsWellKnownSid(sid, WinLocalSystemSid) && !found_system) {
            found_system = TRUE;
        } else if (IsWellKnownSid(sid, WinBuiltinAdministratorsSid) &&
                !found_admins) {
            found_admins = TRUE;
        } else {
            goto done;
        }
    }
    valid = found_system && found_admins;
done:
    LocalFree(descriptor);
    if (!valid) {
        SetLastError(ERROR_ACCESS_DENIED);
    }
    return valid;
}

/* Program Files permits ordinary read/execute ACEs, never untrusted mutation. */
static BOOL install_security(HANDLE handle, BOOL ancestor)
{
    PSECURITY_DESCRIPTOR descriptor = NULL;
    PSID owner = NULL;
    PSID group = NULL;
    PACL acl = NULL;
    DWORD result;
    DWORD i;
    BOOL valid = FALSE;
    DWORD forbidden = ancestor ? ANCESTOR_REPLACEMENT_ACCESS :
        DIRECTORY_WRITE_ACCESS;
    result = GetSecurityInfo(handle, SE_FILE_OBJECT,
        OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION |
        DACL_SECURITY_INFORMATION, &owner, &group, &acl, NULL, &descriptor);
    if (result != ERROR_SUCCESS) {
        SetLastError(result);
        return FALSE;
    }
    if (!trusted_sid(owner, TRUE) || !trusted_sid(group, TRUE) ||
            acl == NULL || !IsValidAcl(acl)) {
        goto done;
    }
    for (i = 0; i < acl->AceCount; ++i) {
        ACE_HEADER *header = NULL;
        ACCESS_ALLOWED_ACE *ace;
        PSID sid;
        if (!GetAce(acl, i, (void **)&header)) {
            goto done;
        }
        if ((header->AceFlags & INHERIT_ONLY_ACE) != 0) {
            continue;
        }
        if (header->AceType == ACCESS_DENIED_ACE_TYPE) {
            continue;
        }
        if (header->AceType != ACCESS_ALLOWED_ACE_TYPE ||
                header->AceSize < sizeof(ACCESS_ALLOWED_ACE)) {
            goto done;
        }
        ace = (ACCESS_ALLOWED_ACE *)header;
        if (!allowed_ace_sid(ace, &sid) ||
                (!trusted_sid(sid, TRUE) && (ace->Mask & forbidden) != 0)) {
            goto done;
        }
    }
    valid = TRUE;
done:
    LocalFree(descriptor);
    if (!valid) {
        SetLastError(ERROR_ACCESS_DENIED);
    }
    return valid;
}

static BOOL directory_attributes(HANDLE handle)
{
    FILE_ATTRIBUTE_TAG_INFO attributes;
    if (GetFileType(handle) != FILE_TYPE_DISK ||
            !GetFileInformationByHandleEx(handle, FileAttributeTagInfo,
                &attributes, sizeof(attributes))) {
        return FALSE;
    }
    if ((attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0 ||
            (attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0) {
        SetLastError(ERROR_ACCESS_DENIED);
        return FALSE;
    }
    return TRUE;
}

static void close_directories(HeldDirectories *directories)
{
    while (directories->count != 0) {
        CloseHandle(directories->handles[--directories->count]);
    }
    if (directories->path != NULL) {
        HeapFree(GetProcessHeap(), 0, directories->path);
        directories->path = NULL;
    }
}

static BOOL hold_directory(HeldDirectories *directories,
        DirectoryPolicy policy)
{
    HANDLE handle;
    DWORD error;
    if (directories->count >= MAX_HELD_DIRECTORIES) {
        SetLastError(ERROR_BUFFER_OVERFLOW);
        return FALSE;
    }
    handle = CreateFileW(directories->path, FILE_READ_ATTRIBUTES | READ_CONTROL,
        FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, NULL);
    if (handle == INVALID_HANDLE_VALUE) {
        return FALSE;
    }
    if (!directory_attributes(handle) ||
            (policy == DirectoryPrivate && !exact_security(handle,
                SE_FILE_OBJECT, FILE_ALL_ACCESS,
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)) ||
            (policy == DirectoryInstall && !install_security(handle, FALSE)) ||
            (policy == DirectoryAncestor && !install_security(handle, TRUE))) {
        error = GetLastError();
        CloseHandle(handle);
        SetLastError(error);
        return FALSE;
    }
    directories->handles[directories->count++] = handle;
    return TRUE;
}

/* Only local absolute drive paths; reject UNC, device paths, ADS and dots. */
static BOOL valid_local_path(const WCHAR *path)
{
    size_t length = wcslen(path);
    size_t begin = 3;
    size_t i;
    if (length < 3 || length >= PATH_CAPACITY - 64 ||
            !((path[0] >= L'A' && path[0] <= L'Z') ||
                (path[0] >= L'a' && path[0] <= L'z')) ||
            path[1] != L':' || path[2] != L'\\') {
        return FALSE;
    }
    for (i = 3; i <= length; ++i) {
        WCHAR c = path[i];
        if (c == L'/' || c == L':' || (c != L'\0' && c < 0x20)) {
            return FALSE;
        }
        if (c == L'\\' || c == L'\0') {
            size_t component = i - begin;
            if (component == 0 || (component == 1 && path[begin] == L'.') ||
                    (component == 2 && path[begin] == L'.' &&
                        path[begin + 1] == L'.') ||
                    path[i - 1] == L'.' || path[i - 1] == L' ') {
                return FALSE;
            }
            begin = i + 1;
        }
    }
    return TRUE;
}

/* Hold every ancestor, including volume root, before resolving a child. */
static BOOL hold_known_folder(HeldDirectories *directories,
        REFKNOWNFOLDERID id, BOOL install_root)
{
    WCHAR *known = NULL;
    size_t length;
    size_t i;
    HRESULT result;
    DWORD error;
    ZeroMemory(directories, sizeof(*directories));
    result = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, NULL, &known);
    if (FAILED(result)) {
        SetLastError(HRESULT_CODE(result));
        return FALSE;
    }
    length = wcslen(known);
    while (length > 3 && known[length - 1] == L'\\') {
        known[--length] = L'\0';
    }
    if (!valid_local_path(known)) {
        CoTaskMemFree(known);
        SetLastError(ERROR_BAD_PATHNAME);
        return FALSE;
    }
    directories->path = (WCHAR *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY,
        PATH_CAPACITY * sizeof(WCHAR));
    if (directories->path == NULL) {
        CoTaskMemFree(known);
        SetLastError(ERROR_OUTOFMEMORY);
        return FALSE;
    }
    memcpy(directories->path, known, (length + 1) * sizeof(WCHAR));
    CoTaskMemFree(known);
    for (i = 3; i <= length; ++i) {
        WCHAR saved = directories->path[i];
        if (i != 3 && saved != L'\\' && saved != L'\0') {
            continue;
        }
        directories->path[i] = L'\0';
        if (!hold_directory(directories, install_root ?
                (i == length ? DirectoryInstall : DirectoryAncestor) :
                DirectoryAncestor)) {
            error = GetLastError();
            directories->path[i] = saved;
            close_directories(directories);
            SetLastError(error);
            return FALSE;
        }
        directories->path[i] = saved;
    }
    return TRUE;
}

static BOOL append_path(HeldDirectories *directories, const WCHAR *component)
{
    if (FAILED(StringCchCatW(directories->path, PATH_CAPACITY, L"\\")) ||
            FAILED(StringCchCatW(directories->path, PATH_CAPACITY, component))) {
        SetLastError(ERROR_BUFFER_OVERFLOW);
        return FALSE;
    }
    return TRUE;
}

static BOOL private_child(HeldDirectories *directories, const WCHAR *name,
        BOOL create)
{
    PSECURITY_DESCRIPTOR descriptor = NULL;
    SECURITY_ATTRIBUTES attributes;
    DWORD error;
    BOOL result;
    if (!append_path(directories, name)) {
        return FALSE;
    }
    if (create) {
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(
                kDirectorySddl, SDDL_REVISION_1, &descriptor, NULL)) {
            return FALSE;
        }
        attributes.nLength = sizeof(attributes);
        attributes.lpSecurityDescriptor = descriptor;
        attributes.bInheritHandle = FALSE;
        result = CreateDirectoryW(directories->path, &attributes);
        error = GetLastError();
        LocalFree(descriptor);
        if (!result && error != ERROR_ALREADY_EXISTS) {
            SetLastError(error);
            return FALSE;
        }
    }
    return hold_directory(directories, DirectoryPrivate);
}

static BOOL private_directories(HeldDirectories *directories, BOOL create)
{
    DWORD error;
    if (!hold_known_folder(directories, &FOLDERID_ProgramData, FALSE)) {
        return FALSE;
    }
    if (!private_child(directories, L"Mechrevo", create) ||
            !private_child(directories, L"Console", create) ||
            !append_path(directories, L"maintenance.json")) {
        error = GetLastError();
        close_directories(directories);
        SetLastError(error);
        return FALSE;
    }
    return TRUE;
}

/* Missing install children are allowed; foreign existing children are not. */
static BOOL install_directories(HeldDirectories *directories)
{
    static const WCHAR *children[] = { L"Mechrevo", L"Console" };
    size_t i;
    DWORD error;
    if (!hold_known_folder(directories, &FOLDERID_ProgramFilesX64, TRUE)) {
        return FALSE;
    }
    for (i = 0; i < ARRAY_COUNT(children); ++i) {
        if (!append_path(directories, children[i])) {
            error = GetLastError();
            close_directories(directories);
            SetLastError(error);
            return FALSE;
        }
        if (!hold_directory(directories, DirectoryInstall)) {
            error = GetLastError();
            if (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND) {
                return TRUE;
            }
            close_directories(directories);
            SetLastError(error);
            return FALSE;
        }
    }
    return TRUE;
}

static BOOL fixed_install_target(MSIHANDLE installer)
{
    WCHAR *known = NULL;
    WCHAR *expected = NULL;
    WCHAR *actual = NULL;
    DWORD capacity = PATH_CAPACITY;
    HRESULT result;
    UINT msi_result;
    size_t length;
    BOOL valid = FALSE;
    DWORD error = ERROR_INVALID_DATA;
    result = SHGetKnownFolderPath(&FOLDERID_ProgramFilesX64, KF_FLAG_DEFAULT,
        NULL, &known);
    if (FAILED(result)) {
        SetLastError(HRESULT_CODE(result));
        return FALSE;
    }
    expected = (WCHAR *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY,
        PATH_CAPACITY * sizeof(WCHAR));
    actual = (WCHAR *)HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY,
        PATH_CAPACITY * sizeof(WCHAR));
    if (expected == NULL || actual == NULL) {
        error = ERROR_OUTOFMEMORY;
        goto done;
    }
    length = wcslen(known);
    while (length > 3 && known[length - 1] == L'\\') {
        known[--length] = L'\0';
    }
    if (!valid_local_path(known) || FAILED(StringCchPrintfW(expected,
            PATH_CAPACITY, L"%s\\Mechrevo\\Console", known))) {
        error = ERROR_BAD_PATHNAME;
        goto done;
    }
    msi_result = MsiGetTargetPathW(installer, L"ConsoleInstallFolder", actual,
        &capacity);
    if (msi_result != ERROR_SUCCESS) {
        error = msi_result;
        goto done;
    }
    length = wcslen(actual);
    while (length > 3 && actual[length - 1] == L'\\') {
        actual[--length] = L'\0';
    }
    valid = valid_local_path(actual) && CompareStringOrdinal(actual, -1,
        expected, -1, TRUE) == CSTR_EQUAL;
done:
    CoTaskMemFree(known);
    if (expected != NULL) {
        HeapFree(GetProcessHeap(), 0, expected);
    }
    if (actual != NULL) {
        HeapFree(GetProcessHeap(), 0, actual);
    }
    if (!valid) {
        SetLastError(error);
    }
    return valid;
}

static BOOL elevated_effective_token(void)
{
    HANDLE token = NULL;
    BYTE user_buffer[512];
    DWORD size = 0;
    BOOL admin = FALSE;
    BYTE admin_sid[SECURITY_MAX_SID_SIZE];
    DWORD admin_size = sizeof(admin_sid);
    BOOL result = FALSE;
    DWORD error = ERROR_ACCESS_DENIED;
    if (!OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, TRUE, &token)) {
        error = GetLastError();
        if (error != ERROR_NO_TOKEN ||
                !OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
            return FALSE;
        }
    }
    if (!GetTokenInformation(token, TokenUser, user_buffer, sizeof(user_buffer),
            &size)) {
        error = GetLastError();
        goto done;
    }
    if (IsWellKnownSid(((TOKEN_USER *)user_buffer)->User.Sid, WinLocalSystemSid)) {
        result = TRUE;
        goto done;
    }
    if (!CreateWellKnownSid(WinBuiltinAdministratorsSid, NULL, admin_sid,
            &admin_size) || !CheckTokenMembership(NULL, admin_sid, &admin)) {
        error = GetLastError();
        goto done;
    }
    result = admin;
done:
    CloseHandle(token);
    if (!result) {
        SetLastError(error);
    }
    return result;
}

static HANDLE acquire_mutex(void)
{
    PSECURITY_DESCRIPTOR descriptor = NULL;
    SECURITY_ATTRIBUTES attributes;
    HANDLE mutex;
    DWORD wait;
    DWORD error;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(kMutexSddl,
            SDDL_REVISION_1, &descriptor, NULL)) {
        return NULL;
    }
    attributes.nLength = sizeof(attributes);
    attributes.lpSecurityDescriptor = descriptor;
    attributes.bInheritHandle = FALSE;
    mutex = CreateMutexExW(&attributes, kMutexName, 0, MUTEX_ACCESS);
    error = GetLastError();
    LocalFree(descriptor);
    if (mutex == NULL) {
        SetLastError(error);
        return NULL;
    }
    if (!exact_security(mutex, SE_KERNEL_OBJECT, MUTEX_ACCESS, 0)) {
        error = GetLastError();
        CloseHandle(mutex);
        SetLastError(error);
        return NULL;
    }
    wait = WaitForSingleObject(mutex, 0);
    if (wait == WAIT_OBJECT_0) {
        return mutex;
    }
    if (wait == WAIT_ABANDONED) {
        /* WAIT_ABANDONED grants ownership: release it, but never trust state. */
        ReleaseMutex(mutex);
        error = ERROR_ABANDONED_WAIT_0;
    } else if (wait == WAIT_TIMEOUT) {
        error = ERROR_BUSY;
    } else {
        error = GetLastError();
    }
    CloseHandle(mutex);
    SetLastError(error);
    return NULL;
}

static BOOL marker_attributes(HANDLE file)
{
    FILE_ATTRIBUTE_TAG_INFO attributes;
    FILE_STANDARD_INFO standard;
    if (GetFileType(file) != FILE_TYPE_DISK ||
            !GetFileInformationByHandleEx(file, FileAttributeTagInfo,
                &attributes, sizeof(attributes)) ||
            !GetFileInformationByHandleEx(file, FileStandardInfo,
                &standard, sizeof(standard))) {
        return FALSE;
    }
    if ((attributes.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT |
            FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_DEVICE)) != 0 ||
            standard.Directory || standard.DeletePending ||
            standard.NumberOfLinks != 1) {
        SetLastError(ERROR_ACCESS_DENIED);
        return FALSE;
    }
    return exact_security(file, SE_FILE_OBJECT, FILE_ALL_ACCESS, 0);
}

static BOOL literal(const char **cursor, const char *expected)
{
    size_t length = strlen(expected);
    if (strncmp(*cursor, expected, length) != 0) {
        return FALSE;
    }
    *cursor += length;
    return TRUE;
}

static BOOL quoted(const char **cursor, char *output, size_t capacity)
{
    size_t length = 0;
    if (**cursor != '"') {
        return FALSE;
    }
    ++*cursor;
    while (**cursor != '"') {
        unsigned char value = (unsigned char)**cursor;
        if (value < 0x20 || value > 0x7e || value == '\\' ||
                length + 1 >= capacity) {
            return FALSE;
        }
        output[length++] = **cursor;
        ++*cursor;
    }
    ++*cursor;
    output[length] = '\0';
    return TRUE;
}

/* Canonical, bounded schema: no duplicate/unknown fields, escapes or suffix. */
static BOOL parse_marker(const char *json, Marker *marker)
{
    const char *cursor = json;
    return literal(&cursor, "{\"schema_version\":1,\"transaction_nonce\":") &&
        quoted(&cursor, marker->transaction.nonce,
            sizeof(marker->transaction.nonce)) &&
        literal(&cursor, ",\"product_code\":") &&
        quoted(&cursor, marker->transaction.product,
            sizeof(marker->transaction.product)) &&
        literal(&cursor, ",\"source_commit\":") &&
        quoted(&cursor, marker->source, sizeof(marker->source)) &&
        literal(&cursor, ",\"release_id\":") &&
        quoted(&cursor, marker->release, sizeof(marker->release)) &&
        literal(&cursor, ",\"operation\":") &&
        quoted(&cursor, marker->transaction.mode,
            sizeof(marker->transaction.mode)) &&
        literal(&cursor, "}\n") && *cursor == '\0' &&
        lowercase_hex(marker->transaction.nonce, 64) &&
        product_code(marker->transaction.product) &&
        lowercase_hex(marker->source, 40) && release_id(marker->release) &&
        ordinary_mode(marker->transaction.mode);
}

static BOOL read_marker(HANDLE file, Marker *marker)
{
    char json[MAX_GATE_BYTES + 1];
    LARGE_INTEGER size;
    LARGE_INTEGER beginning;
    DWORD read = 0;
    DWORD extra = 0;
    char trailing;
    beginning.QuadPart = 0;
    if (!marker_attributes(file) || !GetFileSizeEx(file, &size) ||
            size.QuadPart <= 0 || size.QuadPart > MAX_GATE_BYTES ||
            !SetFilePointerEx(file, beginning, NULL, FILE_BEGIN)) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    if (!ReadFile(file, json, (DWORD)size.QuadPart, &read, NULL) ||
            read != (DWORD)size.QuadPart ||
            !ReadFile(file, &trailing, 1, &extra, NULL) || extra != 0) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    json[read] = '\0';
    if (memchr(json, '\0', read) != NULL || !parse_marker(json, marker)) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    return TRUE;
}

static BOOL write_marker(HANDLE file, const Transaction *transaction)
{
    char json[MAX_GATE_BYTES + 1];
    LARGE_INTEGER beginning;
    size_t length;
    size_t offset = 0;
    Marker verification;
    beginning.QuadPart = 0;
    if (FAILED(StringCchPrintfA(json, ARRAY_COUNT(json),
            "{\"schema_version\":1,\"transaction_nonce\":\"%s\","
            "\"product_code\":\"%s\",\"source_commit\":\"%s\","
            "\"release_id\":\"%s\",\"operation\":\"%s\"}\n",
            transaction->nonce, transaction->product, kSourceCommit,
            kReleaseId, transaction->mode)) ||
            !SetFilePointerEx(file, beginning, NULL, FILE_BEGIN)) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    length = strlen(json);
    while (offset < length) {
        DWORD written = 0;
        if (!WriteFile(file, json + offset, (DWORD)(length - offset),
                &written, NULL) || written == 0) {
            SetLastError(ERROR_WRITE_FAULT);
            return FALSE;
        }
        offset += written;
    }
    if (!SetEndOfFile(file) || !FlushFileBuffers(file) ||
            !read_marker(file, &verification) ||
            strcmp(verification.transaction.nonce, transaction->nonce) != 0 ||
            strcmp(verification.transaction.product, transaction->product) != 0 ||
            strcmp(verification.transaction.mode, transaction->mode) != 0 ||
            strcmp(verification.source, kSourceCommit) != 0 ||
            strcmp(verification.release, kReleaseId) != 0) {
        SetLastError(ERROR_INVALID_DATA);
        return FALSE;
    }
    return TRUE;
}

__declspec(dllexport) UINT __stdcall PrepareMaintenance(MSIHANDLE installer)
{
    SYSTEM_INFO native_system;
    BYTE random[32];
    static const char hexadecimal[] = "0123456789abcdef";
    Transaction transaction;
    WCHAR product[64];
    WCHAR installed[512];
    WCHAR remove[512];
    WCHAR reinstall[512];
    WCHAR upgrade_detected[512];
    WCHAR upgrading_product[512];
    WCHAR rollback_disabled[32];
    WCHAR data[128];
    size_t i;
    UINT result;
    GetNativeSystemInfo(&native_system);
    if (native_system.wProcessorArchitecture != PROCESSOR_ARCHITECTURE_AMD64) {
        return fail(installer, L"native AMD64 operating system", ERROR_NOT_SUPPORTED);
    }
    if (!lowercase_hex(kSourceCommit, 40) || !release_id(kReleaseId)) {
        return fail(installer, L"compiled provenance", ERROR_INVALID_DATA);
    }
    if (!read_property(installer, L"ProductCode", product,
            (DWORD)ARRAY_COUNT(product)) ||
            !read_property(installer, L"Installed", installed,
                (DWORD)ARRAY_COUNT(installed)) ||
            !read_property(installer, L"REMOVE", remove,
                (DWORD)ARRAY_COUNT(remove)) ||
            !read_property(installer, L"REINSTALL", reinstall,
                (DWORD)ARRAY_COUNT(reinstall)) ||
            !read_property(installer, L"WIX_UPGRADE_DETECTED", upgrade_detected,
                (DWORD)ARRAY_COUNT(upgrade_detected)) ||
            !read_property(installer, L"UPGRADINGPRODUCTCODE", upgrading_product,
                (DWORD)ARRAY_COUNT(upgrading_product)) ||
            !read_property(installer, L"RollbackDisabled", rollback_disabled,
                (DWORD)ARRAY_COUNT(rollback_disabled))) {
        return fail(installer, L"MSI properties", GetLastError());
    }
    if (rollback_disabled[0] != L'\0') {
        return fail(installer, L"rollback is disabled", ERROR_ACCESS_DISABLED_BY_POLICY);
    }
    if (!narrow_ascii(product, transaction.product,
            sizeof(transaction.product)) || !product_code(transaction.product)) {
        return fail(installer, L"ProductCode", ERROR_INVALID_DATA);
    }
    if (!fixed_install_target(installer)) {
        return fail(installer, L"fixed Program Files target", GetLastError());
    }
    if (upgrading_product[0] != L'\0') {
        strcpy_s(transaction.mode, sizeof(transaction.mode), "nested");
    } else if (remove[0] != L'\0') {
        strcpy_s(transaction.mode, sizeof(transaction.mode), "remove");
    } else if (installed[0] != L'\0' && reinstall[0] != L'\0') {
        strcpy_s(transaction.mode, sizeof(transaction.mode), "repair");
    } else if (upgrade_detected[0] != L'\0') {
        strcpy_s(transaction.mode, sizeof(transaction.mode), "upgrade");
    } else if (installed[0] != L'\0') {
        return fail(installer, L"unsupported maintenance mode", ERROR_INVALID_DATA);
    } else {
        strcpy_s(transaction.mode, sizeof(transaction.mode), "install");
    }
    if (BCryptGenRandom(NULL, random, (ULONG)sizeof(random),
            BCRYPT_USE_SYSTEM_PREFERRED_RNG) < 0) {
        return fail(installer, L"transaction nonce", ERROR_GEN_FAILURE);
    }
    for (i = 0; i < sizeof(random); ++i) {
        transaction.nonce[i * 2] = hexadecimal[random[i] >> 4];
        transaction.nonce[i * 2 + 1] = hexadecimal[random[i] & 15];
    }
    transaction.nonce[64] = '\0';
    SecureZeroMemory(random, sizeof(random));
    if (FAILED(StringCchPrintfW(data, ARRAY_COUNT(data), L"%hs|%hs|%hs",
            transaction.nonce, transaction.product, transaction.mode))) {
        return fail(installer, L"CustomActionData encoding", ERROR_INVALID_DATA);
    }
    result = MsiSetPropertyW(installer, L"AcquireMaintenance", data);
    if (result == ERROR_SUCCESS) {
        result = MsiSetPropertyW(installer, L"RollbackMaintenance", data);
    }
    if (result == ERROR_SUCCESS) {
        result = MsiSetPropertyW(installer, L"CommitMaintenance", data);
    }
    SecureZeroMemory(data, sizeof(data));
    if (result != ERROR_SUCCESS) {
        return fail(installer, L"deferred action properties", result);
    }
    note(installer, L"Mechrevo maintenance guard: prepared fixed-path transaction.");
    return ERROR_SUCCESS;
}

/*
 * Must be immediate in the execute sequence, after script execution and the
 * nested RemoveExistingProducts action. A deferred call would return FALSE
 * for these reboot modes irrespective of the real flags; reject that misuse.
 * ReplacedInUseFiles additionally catches a session's actual file replacement.
 * This does not claim to inspect or repair global pending-rename operations.
 */
__declspec(dllexport) UINT __stdcall VerifyMaintenance(MSIHANDLE installer)
{
    WCHAR replaced[32];
    WCHAR product[64];
    char product_ascii[39];
    if (MsiGetMode(installer, MSIRUNMODE_SCHEDULED) ||
            MsiGetMode(installer, MSIRUNMODE_ROLLBACK) ||
            MsiGetMode(installer, MSIRUNMODE_COMMIT)) {
        return fail(installer, L"reboot verification requires immediate context",
            ERROR_INVALID_FUNCTION);
    }
    if (!read_property(installer, L"ProductCode", product,
            (DWORD)ARRAY_COUNT(product)) ||
            !narrow_ascii(product, product_ascii, sizeof(product_ascii)) ||
            !product_code(product_ascii) ||
            !read_property(installer, L"ReplacedInUseFiles", replaced,
                (DWORD)ARRAY_COUNT(replaced))) {
        return fail(installer, L"reboot verification session", ERROR_INVALID_DATA);
    }
    if (MsiGetMode(installer, MSIRUNMODE_REBOOTATEND) ||
            MsiGetMode(installer, MSIRUNMODE_REBOOTNOW) || replaced[0] != L'\0') {
        return fail(installer, L"in-use files or session reboot required",
            ERROR_BUSY);
    }
    note(installer, L"Mechrevo maintenance guard: immediate post-execute session "
        L"has no reboot flags or ReplacedInUseFiles.");
    return ERROR_SUCCESS;
}

__declspec(dllexport) UINT __stdcall AcquireMaintenance(MSIHANDLE installer)
{
    Transaction transaction;
    HeldDirectories private_paths;
    HeldDirectories install_paths;
    HANDLE mutex = NULL;
    HANDLE file = INVALID_HANDLE_VALUE;
    PSECURITY_DESCRIPTOR descriptor = NULL;
    SECURITY_ATTRIBUTES attributes;
    Marker marker;
    BOOL nested;
    BOOL repair;
    BOOL existed = FALSE;
    DWORD error = ERROR_SUCCESS;
    const WCHAR *stage = L"transaction input";
    UINT result = ERROR_INSTALL_FAILURE;
    ZeroMemory(&private_paths, sizeof(private_paths));
    ZeroMemory(&install_paths, sizeof(install_paths));
    if (!MsiGetMode(installer, MSIRUNMODE_SCHEDULED) ||
            MsiGetMode(installer, MSIRUNMODE_ROLLBACK) ||
            MsiGetMode(installer, MSIRUNMODE_COMMIT)) {
        return fail(installer, L"Acquire requires deferred execution",
            ERROR_INVALID_FUNCTION);
    }
    if (!lowercase_hex(kSourceCommit, 40) || !release_id(kReleaseId) ||
            !transaction_data(installer, &transaction)) {
        return fail(installer, stage, ERROR_INVALID_DATA);
    }
    if (!elevated_effective_token()) {
        return fail(installer, L"NoImpersonate administrator token", GetLastError());
    }
    nested = strcmp(transaction.mode, "nested") == 0;
    repair = strcmp(transaction.mode, "repair") == 0;
    stage = L"writer mutex (busy writers are not terminated)";
    mutex = acquire_mutex();
    if (mutex == NULL) {
        return fail(installer, stage, GetLastError());
    }
    stage = L"Program Files directory identity and permissions";
    if (!install_directories(&install_paths)) {
        goto done;
    }
    stage = L"ProgramData private directory identity and permissions";
    if (!private_directories(&private_paths, !nested)) {
        goto done;
    }
    stage = L"maintenance marker creation";
    if (nested) {
        file = CreateFileW(private_paths.path, GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT,
            NULL);
        existed = TRUE;
    } else {
        if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(kFileSddl,
                SDDL_REVISION_1, &descriptor, NULL)) {
            goto done;
        }
        attributes.nLength = sizeof(attributes);
        attributes.lpSecurityDescriptor = descriptor;
        attributes.bInheritHandle = FALSE;
        file = CreateFileW(private_paths.path,
            GENERIC_READ | GENERIC_WRITE | READ_CONTROL | DELETE,
            FILE_SHARE_READ, &attributes, CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, NULL);
        error = GetLastError();
        if (file == INVALID_HANDLE_VALUE && repair &&
                (error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS)) {
            file = CreateFileW(private_paths.path,
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL | DELETE,
                FILE_SHARE_READ, NULL, OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT, NULL);
            existed = TRUE;
        } else if (file == INVALID_HANDLE_VALUE) {
            SetLastError(error);
        }
    }
    if (file == INVALID_HANDLE_VALUE) {
        goto done;
    }
    stage = L"maintenance marker identity, ACL and schema";
    if (existed) {
        if (!read_marker(file, &marker)) {
            goto done;
        }
        if (nested) {
            note(installer, L"Mechrevo maintenance guard: nested upgrade borrows "
                L"a valid outer marker; ownership is not transferred.");
            result = ERROR_SUCCESS;
            goto done;
        }
        if (strcmp(marker.transaction.product, transaction.product) != 0) {
            SetLastError(ERROR_ACCESS_DENIED);
            goto done;
        }
    } else if (!marker_attributes(file)) {
        goto done;
    }
    stage = existed ? L"same-product repair marker replacement" :
        L"maintenance marker complete write and flush";
    if (!write_marker(file, &transaction)) {
        goto done;
    }
    note(installer, L"Mechrevo maintenance guard: persistent exclusion marker "
        L"verified; no hardware operation was performed.");
    result = ERROR_SUCCESS;
done:
    error = GetLastError();
    if (file != INVALID_HANDLE_VALUE) {
        CloseHandle(file);
    }
    if (descriptor != NULL) {
        LocalFree(descriptor);
    }
    close_directories(&private_paths);
    close_directories(&install_paths);
    if (!ReleaseMutex(mutex) && result == ERROR_SUCCESS) {
        error = GetLastError();
        stage = L"writer mutex release";
        result = ERROR_INSTALL_FAILURE;
    }
    CloseHandle(mutex);
    if (result != ERROR_SUCCESS) {
        return fail(installer, stage, error);
    }
    return result;
}

__declspec(dllexport) UINT __stdcall ReleaseMaintenance(MSIHANDLE installer)
{
    Transaction transaction;
    HeldDirectories directories;
    HANDLE mutex;
    HANDLE file = INVALID_HANDLE_VALUE;
    Marker marker;
    FILE_DISPOSITION_INFO disposition;
    BOOL rollback;
    BOOL commit;
    DWORD error;
    const WCHAR *stage = L"release transaction input";
    UINT result = ERROR_INSTALL_FAILURE;
    ZeroMemory(&directories, sizeof(directories));
    rollback = MsiGetMode(installer, MSIRUNMODE_ROLLBACK);
    commit = MsiGetMode(installer, MSIRUNMODE_COMMIT);
    if ((!rollback && !commit) || (rollback && commit)) {
        return fail(installer, L"Release requires rollback or commit execution",
            ERROR_INVALID_FUNCTION);
    }
    if (!transaction_data(installer, &transaction)) {
        return fail(installer, stage, GetLastError());
    }
    if (!elevated_effective_token()) {
        return fail(installer, L"NoImpersonate administrator token", GetLastError());
    }
    stage = L"release writer mutex";
    mutex = acquire_mutex();
    if (mutex == NULL) {
        return fail(installer, stage, GetLastError());
    }
    stage = L"release private directory identity and permissions";
    if (!private_directories(&directories, FALSE)) {
        error = GetLastError();
        if (rollback && strcmp(transaction.mode, "nested") != 0 &&
                (error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND)) {
            /* Rollback can run even if Acquire never reached marker creation. */
            result = ERROR_SUCCESS;
        }
        goto done;
    }
    stage = L"release marker open";
    file = CreateFileW(directories.path, GENERIC_READ | READ_CONTROL | DELETE,
        FILE_SHARE_READ, NULL, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, NULL);
    if (file == INVALID_HANDLE_VALUE) {
        error = GetLastError();
        if (rollback && strcmp(transaction.mode, "nested") != 0 &&
                error == ERROR_FILE_NOT_FOUND) {
            result = ERROR_SUCCESS;
        }
        goto done;
    }
    stage = L"release marker identity, ACL and schema";
    if (!read_marker(file, &marker)) {
        goto done;
    }
    if (strcmp(transaction.mode, "nested") == 0) {
        note(installer, L"Mechrevo maintenance guard: nested release leaves "
            L"the valid outer marker untouched.");
        result = ERROR_SUCCESS;
        goto done;
    }
    stage = L"release marker nonce and ProductCode ownership";
    if (strcmp(marker.transaction.nonce, transaction.nonce) != 0 ||
            strcmp(marker.transaction.product, transaction.product) != 0 ||
            strcmp(marker.transaction.mode, transaction.mode) != 0) {
        SetLastError(ERROR_ACCESS_DENIED);
        goto done;
    }
    stage = L"release verified marker by handle";
    disposition.DeleteFile = TRUE;
    if (!SetFileInformationByHandle(file, FileDispositionInfo,
            &disposition, sizeof(disposition))) {
        goto done;
    }
    note(installer, L"Mechrevo maintenance guard: removed only this transaction's "
        L"verified marker; no hardware operation was performed.");
    result = ERROR_SUCCESS;
done:
    error = GetLastError();
    if (file != INVALID_HANDLE_VALUE) {
        if (!CloseHandle(file) && result == ERROR_SUCCESS) {
            error = GetLastError();
            stage = L"release marker handle close";
            result = ERROR_INSTALL_FAILURE;
        }
    }
    close_directories(&directories);
    if (!ReleaseMutex(mutex) && result == ERROR_SUCCESS) {
        error = GetLastError();
        stage = L"release writer mutex ownership";
        result = ERROR_INSTALL_FAILURE;
    }
    CloseHandle(mutex);
    if (result != ERROR_SUCCESS) {
        return fail(installer, stage, error);
    }
    return result;
}
