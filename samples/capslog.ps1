# Must be Windows PowerShell or PowerShell 7 on Windows
if (-not $IsWindows -and $PSVersionTable.PSEdition -eq 'Core') {
    throw "This script requires Windows."
}

Add-Type -AssemblyName System.Windows.Forms
$winFormsAssembly =
    [System.Windows.Forms.Form].Assembly.Location

#Add-Type -AssemblyName System.Drawing

Add-Type `
    -ReferencedAssemblies $winFormsAssembly `
    -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows.Forms;

public class KeyboardLoggerForm : Form
{
    private const int WM_INPUT       = 0x00FF;
    private const int WM_KEYDOWN     = 0x0100;
    private const int WM_KEYUP       = 0x0101;
    private const int WM_SYSKEYDOWN  = 0x0104;
    private const int WM_SYSKEYUP    = 0x0105;

    private const int VK_SHIFT       = 0x10;
    private const int VK_CONTROL     = 0x11;
    private const int VK_MENU        = 0x12;
    private const int VK_CAPITAL     = 0x14;
    private const int VK_LSHIFT      = 0xA0;
    private const int VK_RSHIFT      = 0xA1;
    private const int VK_LCONTROL    = 0xA2;
    private const int VK_RCONTROL    = 0xA3;
    private const int VK_LMENU       = 0xA4;
    private const int VK_RMENU       = 0xA5;
    private const int VK_LWIN        = 0x5B;
    private const int VK_RWIN        = 0x5C;

    private const uint RID_INPUT      = 0x10000003;
    private const uint RIM_TYPEKEYBOARD = 1;
    private const uint RIDEV_INPUTSINK = 0x00000100;
    private const ushort RI_KEY_BREAK = 0x0001;

    [StructLayout(LayoutKind.Sequential)]
    private struct RAWINPUTDEVICE
    {
        public ushort usUsagePage;
        public ushort usUsage;
        public uint dwFlags;
        public IntPtr hwndTarget;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct RAWINPUTHEADER
    {
        public uint dwType;
        public uint dwSize;
        public IntPtr hDevice;
        public IntPtr wParam;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct RAWKEYBOARD
    {
        public ushort MakeCode;
        public ushort Flags;
        public ushort Reserved;
        public ushort VKey;
        public uint Message;
        public uint ExtraInformation;
    }

    [StructLayout(LayoutKind.Explicit)]
    private struct RAWINPUT
    {
        [FieldOffset(0)]
        public RAWINPUTHEADER header;

        // RAWKEYBOARD begins after the 24-byte RAWINPUTHEADER.
        [FieldOffset(24)]
        public RAWKEYBOARD keyboard;
    }

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool RegisterRawInputDevices(
        [In] RAWINPUTDEVICE[] pRawInputDevices,
        uint uiNumDevices,
        uint cbSize);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern uint GetRawInputData(
        IntPtr hRawInput,
        uint uiCommand,
        IntPtr pData,
        ref uint pcbSize,
        uint cbSizeHeader);

    [DllImport("user32.dll")]
    private static extern short GetKeyState(int nVirtKey);

    public KeyboardLoggerForm()
    {
        Text = "Keyboard Logger";
        Width = 500;
        Height = 200;
        ShowInTaskbar = false;

        // Keep the form visible while testing. Minimize if desired.
        // Hide() can be called after construction for a hidden logger.

        RegisterForRawKeyboard();
    }

    private void RegisterForRawKeyboard()
    {
        var devices = new RAWINPUTDEVICE[1];

        devices[0].usUsagePage = 0x01; // Generic Desktop Controls
        devices[0].usUsage = 0x06;     // Keyboard
        devices[0].dwFlags = RIDEV_INPUTSINK;
        devices[0].hwndTarget = this.Handle;

        bool ok = RegisterRawInputDevices(
            devices,
            (uint)devices.Length,
            (uint)Marshal.SizeOf(typeof(RAWINPUTDEVICE)));

        if (!ok)
        {
            int error = Marshal.GetLastWin32Error();
            Log("RegisterRawInputDevices failed: Win32 error " + error);
        }
        else
        {
            Log("Raw Input registered.");
        }
    }

    protected override void WndProc(ref Message m)
    {
        switch (m.Msg)
        {
            case WM_KEYDOWN:
            case WM_KEYUP:
            case WM_SYSKEYDOWN:
            case WM_SYSKEYUP:
                LogWindowKeyMessage(m.Msg, m.WParam, m.LParam);
                break;

            case WM_INPUT:
                LogRawInput(m.LParam);
                break;
        }

        base.WndProc(ref m);
    }

    private void LogWindowKeyMessage(
        int message,
        IntPtr wParam,
        IntPtr lParam)
    {
        int vk = unchecked((int)wParam.ToInt64());

        long lp = lParam.ToInt64();

        int scanCode = (int)((lp >> 16) & 0xff);
        bool extended = ((lp >> 24) & 1) != 0;
        bool previousDown = ((lp >> 30) & 1) != 0;
        bool transitionUp = ((lp >> 31) & 1) != 0;

        bool isDown =
            message == WM_KEYDOWN ||
            message == WM_SYSKEYDOWN;

        Log(
            "[MSG] " +
            MessageName(message) +
            " vk=" + Hex(vk, 2) +
            " scan=" + Hex(scanCode, 2) +
            " extended=" + extended +
            " previousDown=" + previousDown +
            " transitionUp=" + transitionUp +
            " modifiers={" + ModifierState() + "}" +
            " capsToggle=" + CapsToggle() +
            " firstDown=" + (isDown && !previousDown));
    }

    private void LogRawInput(IntPtr hRawInput)
    {
        uint size = 0;

        uint result = GetRawInputData(
            hRawInput,
            RID_INPUT,
            IntPtr.Zero,
            ref size,
            (uint)Marshal.SizeOf(typeof(RAWINPUTHEADER)));

        if (result == unchecked((uint)-1) || size == 0)
        {
            Log("GetRawInputData(size) failed.");
            return;
        }

        IntPtr buffer = Marshal.AllocHGlobal((int)size);

        try
        {
            result = GetRawInputData(
                hRawInput,
                RID_INPUT,
                buffer,
                ref size,
                (uint)Marshal.SizeOf(typeof(RAWINPUTHEADER)));

            if (result == unchecked((uint)-1))
            {
                Log("GetRawInputData(data) failed.");
                return;
            }

            RAWINPUT raw =
                Marshal.PtrToStructure<RAWINPUT>(buffer);

            if (raw.header.dwType != RIM_TYPEKEYBOARD)
                return;

            RAWKEYBOARD key = raw.keyboard;

            bool isUp =
                (key.Flags & RI_KEY_BREAK) != 0;

            bool isExtended =
                (key.Flags & 0x0002) != 0 ||
                (key.Flags & 0x0004) != 0;

            bool isCaps =
                key.VKey == VK_CAPITAL ||
                key.MakeCode == 0x003A;

            Log(
                "[RAW] " +
                (isUp ? "UP  " : "DOWN") +
                " vkey=" + Hex(key.VKey, 4) +
                " makeCode=" + Hex(key.MakeCode, 4) +
                " flags=" + Hex(key.Flags, 4) +
                " extended=" + isExtended +
                " capsCandidate=" + isCaps +
                " device=" + raw.header.hDevice +
                " modifiers={" + ModifierState() + "}" +
                " capsToggle=" + CapsToggle());
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
    }

    private static string MessageName(int message)
    {
        switch (message)
        {
            case WM_KEYDOWN:    return "WM_KEYDOWN";
            case WM_KEYUP:      return "WM_KEYUP";
            case WM_SYSKEYDOWN: return "WM_SYSKEYDOWN";
            case WM_SYSKEYUP:   return "WM_SYSKEYUP";
            default:            return "UNKNOWN";
        }
    }

    private static string Hex(int value, int digits)
    {
        return "0x" + value.ToString("X" + digits);
    }

    private static bool Down(int vk)
    {
        return (GetKeyState(vk) & 0x8000) != 0;
    }

    private static bool Toggle(int vk)
    {
        return (GetKeyState(vk) & 0x0001) != 0;
    }

    private static bool AnyShift()
    {
        return Down(VK_LSHIFT) || Down(VK_RSHIFT);
    }

    private static bool AnyCtrl()
    {
        return Down(VK_LCONTROL) || Down(VK_RCONTROL);
    }

    private static bool AnyAlt()
    {
        return Down(VK_LMENU) || Down(VK_RMENU);
    }

    private static bool AnyWin()
    {
        return Down(VK_LWIN) || Down(VK_RWIN);
    }

    private static string ModifierState()
    {
        return
            "shift=" + AnyShift() +
            ",ctrl=" + AnyCtrl() +
            ",alt=" + AnyAlt() +
            ",win=" + AnyWin();
    }

    private static bool CapsToggle()
    {
        return Toggle(VK_CAPITAL);
    }

    private static void Log(string text)
    {
        string line =
            DateTime.Now.ToString("HH:mm:ss.fff") +
            " " + text;

        Debug.WriteLine(line);
        Console.WriteLine(line);
    }
}
'@

$form = New-Object KeyboardLoggerForm

try {
    [System.Windows.Forms.Application]::Run($form)
}
finally {
    $form.Dispose()
}
