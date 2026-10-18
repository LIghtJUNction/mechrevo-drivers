#Requires -Version 7.4
<#
.SYNOPSIS
Read-only verification of the fixed Windows x64 console installation.
.DESCRIPTION
Does not start any installed executable or modify hardware, services or files.
The manifest is an installation consistency record, not an independent publisher
pin. A development_candidate result is never evidence of a signed public release.
Requires PowerShell 7.4+ in a native Windows x64 process. This script's Windows
syntax and runtime must be verified on Windows; Linux source checks cannot do so.
Run in a fresh pwsh -NoProfile -File process. The PE check reads the system-selected
RT_MANIFEST #1 resource; it does not enumerate every resource language variant.
Requires one related MSI product registered as installed in the machine context.
MSI component ownership and the cached MSI package are not verified; protected
payload hashes are checked independently against the release manifest.
#>
[CmdletBinding(PositionalBinding = $false)]
param(
    [AllowNull()][string] $ExpectedReleaseId,
    [AllowNull()][string] $ExpectedSourceCommit,
    [AllowNull()][string] $ExpectedMsiVersion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$installation = $null
$document = $null
$exitCode = 1
$report = $null

try {
    if (-not $IsWindows -or
        [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne [System.Runtime.InteropServices.Architecture]::X64 -or
        [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture -ne [System.Runtime.InteropServices.Architecture]::X64) {
        throw 'Verification requires Windows x64 and a native x64 PowerShell process.'
    }
    if ($PSBoundParameters.ContainsKey('ExpectedReleaseId') -and
        ($null -eq $ExpectedReleaseId -or $ExpectedReleaseId -cnotmatch '\A[A-Za-z0-9._-]{1,128}\z')) {
        throw 'ExpectedReleaseId must contain 1 to 128 ASCII letters, digits, dots, underscores or hyphens.'
    }
    if ($PSBoundParameters.ContainsKey('ExpectedSourceCommit') -and
        ($null -eq $ExpectedSourceCommit -or $ExpectedSourceCommit -cnotmatch '\A[0-9a-f]{40}\z')) {
        throw 'ExpectedSourceCommit must be exactly 40 lowercase hexadecimal characters.'
    }
    if ($PSBoundParameters.ContainsKey('ExpectedMsiVersion')) {
        if ($null -eq $ExpectedMsiVersion -or
            $ExpectedMsiVersion -cnotmatch '\A(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,4})\z') {
            throw 'ExpectedMsiVersion must be a canonical three-part MSI version.'
        }
        $msiParts = $ExpectedMsiVersion.Split('.')
        if ([uint32]::Parse($msiParts[0]) -gt 255 -or [uint32]::Parse($msiParts[1]) -gt 255 -or [uint32]::Parse($msiParts[2]) -gt 65535) {
            throw 'ExpectedMsiVersion must fit the MSI limits 255.255.65535.'
        }
    }

    if ($null -ne ('Mechrevo.InstalledVerification.LockedInstallation' -as [type])) {
        throw 'The verifier type is already loaded. Run in a fresh pwsh -NoProfile -File process.'
    }
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Globalization;
using System.IO;
using System.Runtime.InteropServices;
using System.Security.AccessControl;
using System.Security.Cryptography;
using System.Security.Principal;
using System.Text;
using System.Xml;
using Microsoft.Win32.SafeHandles;

[assembly: DefaultDllImportSearchPaths(DllImportSearchPath.System32)]

namespace Mechrevo.InstalledVerification {
    public sealed class PathCheck {
        public string Path { get; set; }
        public string OwnerSid { get; set; }
        public string Protection { get; set; }
    }

    public sealed class ExecutableCheck {
        public string Name { get; set; }
        public string Sha256 { get; set; }
        public string Architecture { get; set; }
        public string ExecutionLevel { get; set; }
        public bool UiAccess { get; set; }
    }

    public sealed class MsiProductCheck {
        public string UpgradeCode { get; set; }
        public string ProductCode { get; set; }
        public string ProductName { get; set; }
        public string Version { get; set; }
        public string State { get; set; }
    }

    // Deliberately no caller-supplied installation path or manifest path.
    public sealed class LockedInstallation : IDisposable {
        const uint ReadControl = 0x00020000;
        const uint ReadAttributes = 0x00000080;
        const uint GenericRead = 0x80000000;
        const uint ShareRead = 0x00000001;
        const uint OpenExisting = 3;
        const uint OpenReparsePoint = 0x00200000;
        const uint BackupSemantics = 0x02000000;
        const uint ReparsePoint = 0x00000400;
        const uint DirectoryAttribute = 0x00000010;
        // Write data/add file, append/add directory, EA, attributes, delete child,
        // DELETE, WRITE_DAC, WRITE_OWNER, GENERIC_ALL and GENERIC_WRITE.
        const uint MutatingAccess = 0x500D0156;
        static readonly HashSet<string> TrustedOwners = new HashSet<string>(StringComparer.Ordinal) {
            "S-1-5-18", "S-1-5-32-544",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
        };
        static readonly string[] RequiredFiles = {
            "console-release.json", "mechrevo-console.exe",
            "mechrevo-console-gui.exe", "mechrevo-console-oem-helper.exe"
        };
        readonly List<SafeFileHandle> directories = new List<SafeFileHandle>();
        readonly Dictionary<string, FileStream> files = new Dictionary<string, FileStream>(StringComparer.Ordinal);
        readonly List<PathCheck> pathChecks = new List<PathCheck>();
        bool disposed;

        public string InstallationRoot { get; private set; }
        public PathCheck[] Paths { get { return pathChecks.ToArray(); } }

        [StructLayout(LayoutKind.Sequential)]
        struct FileInformation {
            public uint Attributes;
            public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
            public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
            public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
            public uint VolumeSerialNumber;
            public uint FileSizeHigh;
            public uint FileSizeLow;
            public uint NumberOfLinks;
            public uint FileIndexHigh;
            public uint FileIndexLow;
        }

        [DllImport("shell32.dll", ExactSpelling = true)]
        static extern int SHGetKnownFolderPath(ref Guid id, uint flags, IntPtr token, out IntPtr path);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        static extern SafeFileHandle CreateFileW(string path, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
        [DllImport("kernel32.dll", ExactSpelling = true, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool GetFileInformationByHandle(SafeFileHandle file, out FileInformation information);
        [DllImport("kernel32.dll", ExactSpelling = true, SetLastError = true)]
        static extern uint GetFileType(SafeFileHandle file);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        static extern uint GetFinalPathNameByHandleW(SafeFileHandle file, StringBuilder path, uint length, uint flags);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        static extern uint GetDriveTypeW(string root);
        [DllImport("advapi32.dll", ExactSpelling = true)]
        static extern uint GetSecurityInfo(SafeFileHandle handle, int objectType, uint securityInformation,
            out IntPtr owner, out IntPtr group, out IntPtr dacl, out IntPtr sacl, out IntPtr descriptor);
        [DllImport("advapi32.dll", ExactSpelling = true)]
        static extern uint GetSecurityDescriptorLength(IntPtr descriptor);
        [DllImport("kernel32.dll", ExactSpelling = true)]
        static extern IntPtr LocalFree(IntPtr memory);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
        static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);
        [DllImport("kernel32.dll", ExactSpelling = true, SetLastError = true)]
        static extern uint SizeofResource(IntPtr module, IntPtr resource);
        [DllImport("kernel32.dll", ExactSpelling = true, SetLastError = true)]
        static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
        [DllImport("kernel32.dll", ExactSpelling = true)]
        static extern IntPtr LockResource(IntPtr resource);
        [DllImport("kernel32.dll", ExactSpelling = true, SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        static extern bool FreeLibrary(IntPtr module);
        [DllImport("msi.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        static extern uint MsiEnumRelatedProductsW(string upgradeCode, uint reserved, uint index, StringBuilder productCode);
        [DllImport("msi.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
        static extern uint MsiGetProductInfoExW(string productCode, string userSid, uint context,
            string property, StringBuilder value, ref uint length);

        static string MachineProductProperty(string productCode, string property) {
            // MSIINSTALLCONTEXT_MACHINE = 4; a null SID is mandatory here.
            // These are registration reads, never configure/install/repair calls.
            StringBuilder value = new StringBuilder(256);
            uint length = (uint)value.Capacity;
            uint result = MsiGetProductInfoExW(productCode, null, 4, property, value, ref length);
            if (result == 234) { // ERROR_MORE_DATA; returned length excludes NUL.
                if (length >= 4096) throw new InvalidDataException("MSI registration property is too large: " + property);
                value = new StringBuilder(checked((int)length + 1));
                length = (uint)value.Capacity;
                result = MsiGetProductInfoExW(productCode, null, 4, property, value, ref length);
            }
            if (result != 0) throw new Win32Exception((int)result, "Read machine MSI registration property: " + property);
            return value.ToString();
        }

        static bool IsCanonicalMsiVersion(string version) {
            if (String.IsNullOrEmpty(version)) return false;
            string[] parts = version.Split('.');
            if (parts.Length != 3) return false;
            uint[] limits = { 255, 255, 65535 };
            for (int i = 0; i < 3; i++) {
                uint number;
                if (!UInt32.TryParse(parts[i], NumberStyles.None, CultureInfo.InvariantCulture, out number) ||
                    number > limits[i] || !String.Equals(parts[i], number.ToString(CultureInfo.InvariantCulture), StringComparison.Ordinal)) return false;
            }
            return true;
        }

        public static MsiProductCheck VerifyMachineProduct(string expectedVersion) {
            const string upgradeCode = "{31B9D6D6-60C5-4F6D-A2C9-4D352CE44789}";
            const string expectedName = "Mechrevo Console";
            // MsiEnumRelatedProducts covers products visible for the current
            // user and machine, not an inventory of every other user's products.
            StringBuilder product = new StringBuilder(39);
            uint result = MsiEnumRelatedProductsW(upgradeCode, 0, 0, product);
            if (result == 259) throw new InvalidDataException("No related MSI product is registered; copied payloads are not an MSI installation.");
            if (result != 0) throw new Win32Exception((int)result, "Enumerate related MSI product");
            string productCode = product.ToString();
            Guid parsed;
            if (productCode.Length != 38 || !Guid.TryParseExact(productCode, "B", out parsed))
                throw new InvalidDataException("MSI returned an invalid ProductCode.");
            product.Clear();
            result = MsiEnumRelatedProductsW(upgradeCode, 0, 1, product);
            if (result == 0) throw new InvalidDataException("Multiple related MSI products are registered; installation identity is ambiguous.");
            if (result != 259) throw new Win32Exception((int)result, "Finish related MSI product enumeration");
            string state = MachineProductProperty(productCode, "State");
            if (!String.Equals(state, "5", StringComparison.Ordinal))
                throw new InvalidDataException("The related MSI product must be installed, not merely advertised.");
            string name = MachineProductProperty(productCode, "ProductName");
            string installedName = MachineProductProperty(productCode, "InstalledProductName");
            if (!String.Equals(name, expectedName, StringComparison.Ordinal) || !String.Equals(installedName, expectedName, StringComparison.Ordinal))
                throw new InvalidDataException("MSI product name differs from Mechrevo Console.");
            if (!String.Equals(MachineProductProperty(productCode, "AssignmentType"), "1", StringComparison.Ordinal))
                throw new InvalidDataException("The MSI product is not assigned per machine.");
            string version = MachineProductProperty(productCode, "VersionString");
            if (!IsCanonicalMsiVersion(version)) throw new InvalidDataException("MSI VersionString is not a canonical three-part MSI version.");
            if (!String.IsNullOrEmpty(expectedVersion) && !String.Equals(version, expectedVersion, StringComparison.Ordinal))
                throw new InvalidDataException("Installed MSI version differs from ExpectedMsiVersion.");
            return new MsiProductCheck { UpgradeCode = upgradeCode, ProductCode = productCode,
                ProductName = name, Version = version, State = "installed" };
        }

        static Exception NativeError(string operation) {
            return new Win32Exception(Marshal.GetLastWin32Error(), operation);
        }

        static string NormalizePath(string path) {
            return System.IO.Path.GetFullPath(path).TrimEnd('\\');
        }

        static string FixedProgramFiles() {
            Guid id = new Guid("6D809377-6AF0-444b-8957-A3773F02200E");
            IntPtr value;
            int result = SHGetKnownFolderPath(ref id, 0, IntPtr.Zero, out value);
            if (result < 0) Marshal.ThrowExceptionForHR(result);
            try {
                string path = Marshal.PtrToStringUni(value);
                if (String.IsNullOrEmpty(path) || path.Length < 3 ||
                    !((path[0] >= 'A' && path[0] <= 'Z') || (path[0] >= 'a' && path[0] <= 'z')) ||
                    path[1] != ':' || path[2] != '\\') {
                    throw new InvalidDataException("ProgramFilesX64 is not an absolute local drive path.");
                }
                return System.IO.Path.GetFullPath(path);
            } finally { if (value != IntPtr.Zero) Marshal.FreeCoTaskMem(value); }
        }

        static string CheckOwnerAndDacl(SafeFileHandle handle) {
            IntPtr owner, group, dacl, sacl, descriptor;
            uint error = GetSecurityInfo(handle, 1, 0x00000001 | 0x00000004,
                out owner, out group, out dacl, out sacl, out descriptor);
            if (error != 0) throw new Win32Exception((int)error, "GetSecurityInfo");
            try {
                uint length = GetSecurityDescriptorLength(descriptor);
                if (length == 0 || length > 1024 * 1024) throw new InvalidDataException("Invalid security descriptor length.");
                byte[] bytes = new byte[checked((int)length)];
                Marshal.Copy(descriptor, bytes, 0, bytes.Length);
                RawSecurityDescriptor security = new RawSecurityDescriptor(bytes, 0);
                if (security.Owner == null || !TrustedOwners.Contains(security.Owner.Value))
                    throw new InvalidDataException("Owner must be SYSTEM, Administrators or TrustedInstaller.");
                if ((security.ControlFlags & ControlFlags.DiscretionaryAclPresent) == 0 || security.DiscretionaryAcl == null)
                    throw new InvalidDataException("Missing or NULL DACL is not protected.");
                foreach (GenericAce ace in security.DiscretionaryAcl) {
                    // An inherit-only ACE grants no access to this object. Every
                    // existing child is independently checked through its handle.
                    if ((ace.AceFlags & AceFlags.InheritOnly) != 0) continue;
                    QualifiedAce qualified = ace as QualifiedAce;
                    if (qualified == null) throw new InvalidDataException("Unsupported DACL ACE; cannot establish protection.");
                    if (qualified.AceQualifier == AceQualifier.AccessDenied) continue;
                    if (qualified.AceQualifier != AceQualifier.AccessAllowed)
                        throw new InvalidDataException("Unsupported DACL qualifier.");
                    // Deny ACEs are deliberately not used to subtract access:
                    // rejecting a possible writer is conservative and fail closed.
                    if ((unchecked((uint)qualified.AccessMask) & MutatingAccess) != 0 &&
                        !TrustedOwners.Contains(qualified.SecurityIdentifier.Value)) {
                        throw new InvalidDataException("DACL grants mutation rights to an untrusted SID: " + qualified.SecurityIdentifier.Value);
                    }
                }
                return security.Owner.Value;
            } finally { if (descriptor != IntPtr.Zero) LocalFree(descriptor); }
        }

        static SafeFileHandle OpenChecked(string path, bool directory, bool protectAcl, out string owner) {
            SafeFileHandle handle = CreateFileW(path, (directory ? ReadAttributes : GenericRead) | ReadControl,
                ShareRead, IntPtr.Zero, OpenExisting, OpenReparsePoint | (directory ? BackupSemantics : 0), IntPtr.Zero);
            if (handle.IsInvalid) { handle.Dispose(); throw NativeError("Open protected installation object: " + path); }
            try {
                FileInformation info;
                if (!GetFileInformationByHandle(handle, out info)) throw NativeError("GetFileInformationByHandle");
                if ((info.Attributes & ReparsePoint) != 0) throw new InvalidDataException("Reparse points are forbidden: " + path);
                if (((info.Attributes & DirectoryAttribute) != 0) != directory) throw new InvalidDataException("Wrong installation object type: " + path);
                if (GetFileType(handle) != 1) throw new InvalidDataException("Only disk objects are allowed.");
                StringBuilder resolved = new StringBuilder(32768);
                uint count = GetFinalPathNameByHandleW(handle, resolved, (uint)resolved.Capacity, 0);
                if (count == 0) throw NativeError("GetFinalPathNameByHandleW");
                if (count >= resolved.Capacity) throw new InvalidDataException("Resolved path is too long.");
                string actual = resolved.ToString();
                if (!actual.StartsWith(@"\\?\", StringComparison.Ordinal)) throw new InvalidDataException("Unexpected final path form.");
                actual = actual.Substring(4);
                if (!String.Equals(NormalizePath(actual), NormalizePath(path), StringComparison.OrdinalIgnoreCase))
                    throw new InvalidDataException("Installation path resolves to another object: " + path);
                owner = protectAcl ? CheckOwnerAndDacl(handle) : null;
                return handle;
            } catch { handle.Dispose(); throw; }
        }

        public LockedInstallation() {
            try {
                string programFiles = FixedProgramFiles();
                string volumeRoot = System.IO.Path.GetPathRoot(programFiles);
                if (GetDriveTypeW(volumeRoot) != 3) throw new InvalidDataException("ProgramFilesX64 must be on a fixed local drive.");
                if (String.Equals(NormalizePath(programFiles), NormalizePath(volumeRoot), StringComparison.OrdinalIgnoreCase))
                    throw new InvalidDataException("ProgramFilesX64 cannot be the volume root.");
                InstallationRoot = System.IO.Path.Combine(programFiles, "Mechrevo", "Console");
                string owner;
                directories.Add(OpenChecked(volumeRoot, true, false, out owner));
                pathChecks.Add(new PathCheck { Path = volumeRoot, OwnerSid = null, Protection = "fixed_local_volume_no_reparse_handle_held" });
                // The volume root may legitimately permit creation of new sibling
                // folders. Program Files and all further components are strict;
                // intermediate components of a relocated KnownFolder are strict too.
                string relative = InstallationRoot.Substring(volumeRoot.Length);
                string current = volumeRoot;
                foreach (string component in relative.Split(new char[] { '\\' }, StringSplitOptions.RemoveEmptyEntries)) {
                    current = System.IO.Path.Combine(current, component);
                    directories.Add(OpenChecked(current, true, true, out owner));
                    pathChecks.Add(new PathCheck { Path = current, OwnerSid = owner, Protection = "trusted_owner_no_untrusted_mutation_no_reparse_handle_held" });
                }
                // Hold the manifest and ALL three executable handles together.
                // FILE_SHARE_READ excludes existing/future write/delete handles.
                foreach (string name in RequiredFiles) {
                    string path = System.IO.Path.Combine(InstallationRoot, name);
                    SafeFileHandle handle = OpenChecked(path, false, true, out owner);
                    try { files.Add(name, new FileStream(handle, FileAccess.Read, 4096, false)); }
                    catch { handle.Dispose(); throw; }
                    pathChecks.Add(new PathCheck { Path = path, OwnerSid = owner, Protection = "trusted_owner_no_untrusted_mutation_no_reparse_read_only_handle_held" });
                }
            } catch { Dispose(); throw; }
        }

        public byte[] ReadManifest() {
            FileStream stream = files["console-release.json"];
            if (stream.Length == 0 || stream.Length > 65536) throw new InvalidDataException("Release manifest must contain 1 to 65536 bytes.");
            stream.Position = 0;
            byte[] bytes = new byte[checked((int)stream.Length)];
            stream.ReadExactly(bytes, 0, bytes.Length);
            if (bytes.Length >= 3 && bytes[0] == 0xef && bytes[1] == 0xbb && bytes[2] == 0xbf)
                throw new InvalidDataException("UTF-8 BOM is forbidden in console-release.json.");
            return bytes;
        }

        public string HashFile(string name) {
            FileStream stream;
            if (!files.TryGetValue(name, out stream)) throw new InvalidDataException("Not a fixed installation file.");
            stream.Position = 0;
            using (SHA256 sha = SHA256.Create()) {
                return Convert.ToHexString(sha.ComputeHash(stream)).ToLowerInvariant();
            }
        }

        static void RequireAmd64Executable(FileStream stream) {
            if (stream.Length < 64) throw new InvalidDataException("Truncated DOS header.");
            stream.Position = 0;
            using (BinaryReader reader = new BinaryReader(stream, Encoding.UTF8, true)) {
                if (reader.ReadUInt16() != 0x5a4d) throw new InvalidDataException("Missing MZ header.");
                stream.Position = 0x3c;
                int offset = reader.ReadInt32();
                if (offset < 64 || offset > stream.Length - 26) throw new InvalidDataException("Invalid PE header offset.");
                stream.Position = offset;
                if (reader.ReadUInt32() != 0x00004550 || reader.ReadUInt16() != 0x8664)
                    throw new InvalidDataException("Executable must have an AMD64 PE header.");
                stream.Position = offset + 20;
                ushort optionalSize = reader.ReadUInt16();
                ushort characteristics = reader.ReadUInt16();
                if (optionalSize < 2 || offset + 24L + optionalSize > stream.Length ||
                    (characteristics & 0x0002) == 0 || (characteristics & 0x2000) != 0 || reader.ReadUInt16() != 0x020b)
                    throw new InvalidDataException("Expected an executable PE32+ image, not a DLL.");
            }
        }

        static void CheckEmbeddedManifest(string path, string expectedLevel) {
            // Data/resource mappings only: no entry point, DLL initialization,
            // dependency execution, or execution of the inspected program.
            IntPtr module = LoadLibraryExW(path, IntPtr.Zero, 0x00000002 | 0x00000020);
            if (module == IntPtr.Zero) throw NativeError("Map PE as data/image resource");
            try {
                IntPtr resource = FindResourceW(module, new IntPtr(1), new IntPtr(24));
                if (resource == IntPtr.Zero) throw NativeError("Find RT_MANIFEST resource #1");
                uint size = SizeofResource(module, resource);
                if (size == 0 || size > 65536) throw new InvalidDataException("Invalid embedded manifest size.");
                IntPtr loaded = LoadResource(module, resource);
                if (loaded == IntPtr.Zero) throw NativeError("Load manifest resource data");
                IntPtr address = LockResource(loaded);
                if (address == IntPtr.Zero) throw new InvalidDataException("Cannot read embedded manifest resource.");
                byte[] bytes = new byte[checked((int)size)];
                Marshal.Copy(address, bytes, 0, bytes.Length);
                XmlReaderSettings settings = new XmlReaderSettings {
                    DtdProcessing = DtdProcessing.Prohibit, XmlResolver = null,
                    MaxCharactersInDocument = 65536, ConformanceLevel = ConformanceLevel.Document
                };
                XmlDocument document = new XmlDocument { XmlResolver = null };
                using (MemoryStream data = new MemoryStream(bytes, false))
                using (XmlReader reader = XmlReader.Create(data, settings)) document.Load(reader);
                if (document.DocumentElement == null || document.DocumentElement.LocalName != "assembly" ||
                    document.DocumentElement.NamespaceURI != "urn:schemas-microsoft-com:asm.v1")
                    throw new InvalidDataException("Unexpected embedded manifest root.");
                XmlNamespaceManager ns = new XmlNamespaceManager(document.NameTable);
                ns.AddNamespace("a", "urn:schemas-microsoft-com:asm.v1");
                ns.AddNamespace("u", "urn:schemas-microsoft-com:asm.v3");
                XmlNodeList allLevels = document.SelectNodes("//*[local-name()='requestedExecutionLevel']");
                XmlNodeList levels = document.SelectNodes("/a:assembly/u:trustInfo/u:security/u:requestedPrivileges/u:requestedExecutionLevel", ns);
                if (allLevels.Count != 1 || levels.Count != 1) throw new InvalidDataException("Expected exactly one requestedExecutionLevel in the standard manifest location.");
                XmlElement level = (XmlElement)levels[0];
                if (level.GetAttribute("level") != expectedLevel || level.GetAttribute("uiAccess") != "false")
                    throw new InvalidDataException("Unexpected execution level or uiAccess in embedded manifest.");
            } finally { FreeLibrary(module); }
        }

        public ExecutableCheck InspectExecutable(string name, string expectedSha256, string expectedLevel) {
            FileStream stream;
            if (name == "console-release.json" || !files.TryGetValue(name, out stream)) throw new InvalidDataException("Not a fixed executable.");
            string actualHash = HashFile(name);
            if (!String.Equals(actualHash, expectedSha256, StringComparison.Ordinal)) throw new InvalidDataException("SHA-256 mismatch: " + name);
            RequireAmd64Executable(stream);
            CheckEmbeddedManifest(System.IO.Path.Combine(InstallationRoot, name), expectedLevel);
            return new ExecutableCheck { Name = name, Sha256 = actualHash, Architecture = "x86_64", ExecutionLevel = expectedLevel, UiAccess = false };
        }

        public void Dispose() {
            if (disposed) return;
            disposed = true;
            foreach (FileStream file in files.Values) file.Dispose();
            for (int i = directories.Count - 1; i >= 0; i--) directories[i].Dispose();
        }
    }
}
'@

    function Get-StrictObjectProperties {
        param([System.Text.Json.JsonElement] $Element, [string[]] $ExpectedNames)
        if ($Element.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
            throw 'Expected a JSON object.'
        }
        $properties = [System.Collections.Generic.Dictionary[string,System.Text.Json.JsonElement]]::new([System.StringComparer]::Ordinal)
        foreach ($property in $Element.EnumerateObject()) {
            if (-not $properties.TryAdd($property.Name, $property.Value)) {
                throw "Duplicate JSON field: $($property.Name)"
            }
        }
        if ($properties.Count -ne $ExpectedNames.Count) { throw 'Unexpected JSON field count.' }
        foreach ($name in $ExpectedNames) {
            if (-not $properties.ContainsKey($name)) { throw "Missing or unexpected JSON field: $name" }
        }
        # Keep the dictionary as one pipeline object.
        return ,$properties
    }

    function Get-StrictString {
        param([System.Text.Json.JsonElement] $Element, [string] $Name)
        if ($Element.ValueKind -ne [System.Text.Json.JsonValueKind]::String) { throw "$Name must be a JSON string." }
        return $Element.GetString()
    }

    $installation = [Mechrevo.InstalledVerification.LockedInstallation]::new()
    $msiProduct = [Mechrevo.InstalledVerification.LockedInstallation]::VerifyMachineProduct($ExpectedMsiVersion)
    $manifestBytes = $installation.ReadManifest()
    $strictUtf8 = [System.Text.UTF8Encoding]::new($false, $true)
    $manifestText = $strictUtf8.GetString($manifestBytes)
    $jsonOptions = [System.Text.Json.JsonDocumentOptions]::new()
    $jsonOptions.AllowTrailingCommas = $false
    $jsonOptions.CommentHandling = [System.Text.Json.JsonCommentHandling]::Disallow
    $jsonOptions.MaxDepth = 4
    $document = [System.Text.Json.JsonDocument]::Parse($manifestText, $jsonOptions)
    $properties = Get-StrictObjectProperties $document.RootElement @('schema_version', 'release_id', 'source_commit', 'architecture', 'trust', 'files')
    if ($properties['schema_version'].ValueKind -ne [System.Text.Json.JsonValueKind]::Number -or
        $properties['schema_version'].GetRawText() -cne '1') { throw 'schema_version must be the integer 1.' }
    $releaseId = Get-StrictString $properties['release_id'] 'release_id'
    $sourceCommit = Get-StrictString $properties['source_commit'] 'source_commit'
    $architecture = Get-StrictString $properties['architecture'] 'architecture'
    $trust = Get-StrictString $properties['trust'] 'trust'
    if ($releaseId -cnotmatch '\A[A-Za-z0-9._-]{1,128}\z') { throw 'Invalid release_id.' }
    if ($sourceCommit -cnotmatch '\A[0-9a-f]{40}\z') { throw 'Invalid source_commit.' }
    if ($architecture -cne 'x86_64') { throw 'architecture must be x86_64.' }
    if ($trust -cne 'development_candidate' -and $trust -cne 'authenticode') { throw 'Unknown release trust mode.' }
    if ($PSBoundParameters.ContainsKey('ExpectedReleaseId') -and $releaseId -cne $ExpectedReleaseId) { throw 'Installed release_id differs from ExpectedReleaseId.' }
    if ($PSBoundParameters.ContainsKey('ExpectedSourceCommit') -and $sourceCommit -cne $ExpectedSourceCommit) { throw 'Installed source_commit differs from ExpectedSourceCommit.' }

    $names = @('mechrevo-console.exe', 'mechrevo-console-gui.exe', 'mechrevo-console-oem-helper.exe')
    $hashes = Get-StrictObjectProperties $properties['files'] $names
    $results = [System.Collections.Generic.List[object]]::new()
    $signerThumbprint = $null
    foreach ($name in $names) {
        $expectedHash = Get-StrictString $hashes[$name] "files.$name"
        if ($expectedHash -cnotmatch '\A[0-9a-f]{64}\z') { throw "Invalid SHA-256 for $name" }
        $level = if ($name -ceq 'mechrevo-console-oem-helper.exe') { 'requireAdministrator' } else { 'asInvoker' }
        $inspection = $installation.InspectExecutable($name, $expectedHash, $level)
        $signatureStatus = 'not_required_development_candidate'
        $thumbprint = $null
        if ($trust -ceq 'authenticode') {
            $signature = Microsoft.PowerShell.Security\Get-AuthenticodeSignature -LiteralPath ([System.IO.Path]::Combine($installation.InstallationRoot, $name))
            if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or $null -eq $signature.SignerCertificate) {
                throw "A valid Authenticode signature is required for $name"
            }
            # A catalog signature is not evidence of an embedded signed payload.
            if ($signature.SignatureType.ToString() -cne 'Authenticode') { throw "An embedded Authenticode signature is required for $name" }
            $thumbprint = $signature.SignerCertificate.Thumbprint.ToUpperInvariant()
            if ($thumbprint -cnotmatch '\A[0-9A-F]{40}\z') { throw 'Invalid signer thumbprint.' }
            if ($null -eq $signerThumbprint) { $signerThumbprint = $thumbprint }
            elseif ($signerThumbprint -cne $thumbprint) { throw 'All three executables must have the same signer certificate.' }
            $signatureStatus = 'Valid'
        }
        $results.Add([ordered]@{
            name = $inspection.Name
            sha256 = $inspection.Sha256
            architecture = $inspection.Architecture
            execution_level = $inspection.ExecutionLevel
            ui_access = $inspection.UiAccess
            manifest_resource_scope = 'system_selected_RT_MANIFEST_1'
            signature_status = $signatureStatus
            signer_thumbprint = $thumbprint
        })
    }

    $report = [ordered]@{
        schema_version = 1
        status = 'verified'
        scope = 'read_only_fixed_windows_x64_installation'
        installation_root = $installation.InstallationRoot
        release_id = $releaseId
        source_commit = $sourceCommit
        trust = $trust
        release_classification = $(if ($trust -ceq 'development_candidate') { 'development_candidate_not_a_signed_public_release' } else { 'consistent_valid_embedded_signatures_not_an_independent_publisher_pin' })
        manifest_sha256 = $installation.HashFile('console-release.json')
        msi = [ordered]@{
            upgrade_code = $msiProduct.UpgradeCode
            product_code = $msiProduct.ProductCode
            product_name = $msiProduct.ProductName
            version = $msiProduct.Version
            context = 'machine'
            state = $msiProduct.State
            registration_scope = 'unique_related_product_visible_to_current_user_and_machine'
            component_ownership_verified = $false
            cached_package_verified = $false
        }
        path_checks = $installation.Paths
        files = $results.ToArray()
        application_started = $false
        hardware_driver_or_service_modified = $false
        hardware_behavior_verified = $false
    }
    $exitCode = 0
}
catch {
    $report = [ordered]@{
        schema_version = 1
        status = 'failed'
        scope = 'read_only_fixed_windows_x64_installation'
        error = $_.Exception.Message
        application_started = $false
        hardware_driver_or_service_modified = $false
        hardware_behavior_verified = $false
    }
}
finally {
    if ($null -ne $document) { $document.Dispose() }
    if ($null -ne $installation) { $installation.Dispose() }
}

$report | ConvertTo-Json -Depth 8 -Compress
exit $exitCode
