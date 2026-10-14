# 把某个进程的主窗口截图后降采样成 ASCII 亮度图，直接在终端里"看"布局。
#
# 用途：没法看图的时候检查界面（卡片位置、侧栏宽度、间距、控件有没有画出来）。
# 亮度按 0-9 分级输出，相邻灰阶（例如 #F3F3F3 背景 vs #FFFFFF 卡片）也能分辨。
#
# 用法：
#   powershell -File tools\window-ascii.ps1 -Name xunyin-ime -Cols 110 -Rows 45
param(
    [string]$Name = "xunyin-ime",
    [int]$Cols = 110,
    [int]$Rows = 45,
    [switch]$Thresholded
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class WinShot
{
    [DllImport("user32.dll")] private static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    // 本进程必须声明 DPI 感知，否则 GetClientRect/PrintWindow 拿到的是被虚拟化的
    // 坐标（150% 缩放时只有真实尺寸的 2/3），截出来的图会被裁掉右半边。
    [DllImport("user32.dll")] private static extern bool SetProcessDPIAware();
    public static void MakeDpiAware() { SetProcessDPIAware(); }
    [DllImport("user32.dll")] private static extern bool GetClientRect(IntPtr hwnd, out RECT r);
    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);

    public struct RECT { public int Left, Top, Right, Bottom; }

    public static IntPtr HandleOf(uint pid)
    {
        // 找该进程的任意可见顶层窗口
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) =>
        {
            uint p;
            GetWindowThreadProcessId(h, out p);
            if (p == pid)
            {
                RECT r;
                if (GetClientRect(h, out r) && (r.Right - r.Left) > 200) { found = h; return false; }
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    private delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc cb, IntPtr l);

    public static string Ascii(IntPtr hwnd, int cols, int rows, bool thresholded)
    {
        RECT r;
        if (!GetClientRect(hwnd, out r)) return "GetClientRect 失败";
        int w = r.Right - r.Left, h = r.Bottom - r.Top;
        if (w <= 0 || h <= 0) return "窗口尺寸为 0";
        using (var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb))
        {
            using (var g = Graphics.FromImage(bmp))
            {
                IntPtr hdc = g.GetHdc();
                bool ok = PrintWindow(hwnd, hdc, 2); // PW_RENDERFULLCONTENT
                g.ReleaseHdc(hdc);
                if (!ok) return "PrintWindow 失败";
            }
            var sb = new StringBuilder();
            sb.Append("窗口 ").Append(w).Append('x').Append(h).Append('\n');
            for (int ry = 0; ry < rows; ry++)
            {
                for (int rx = 0; rx < cols; rx++)
                {
                    int x0 = rx * w / cols, x1 = Math.Max(x0 + 1, (rx + 1) * w / cols);
                    int y0 = ry * h / rows, y1 = Math.Max(y0 + 1, (ry + 1) * h / rows);
                    long sum = 0; int n = 0;
                    for (int y = y0; y < y1; y++)
                        for (int x = x0; x < x1; x++)
                        {
                            Color c = bmp.GetPixel(x, y);
                            sum += (c.R * 299 + c.G * 587 + c.B * 114) / 1000;
                            n++;
                        }
                    int lum = n > 0 ? (int)(sum / n) : 0;
                    if (thresholded)
                    {
                        // 分级显示：卡片底 / 页面底 / 描边 / 深浅文字各一档，
                        // 这样 #F3F3F3 页面底和 #FFFFFF 卡片能分辨出来
                        char ch;
                        if (lum >= 250) ch = '#';        // 卡片（白）
                        else if (lum >= 238) ch = '.';   // 页面底（#F3F3F3≈243）
                        else if (lum >= 225) ch = ':';   // 描边/浅灰控件
                        else if (lum >= 150) ch = '+';   // 次级文字
                        else if (lum >= 90) ch = '*';    // 正文
                        else ch = '@';                   // 重色/实心
                        sb.Append(ch);
                    }
                    else
                    {
                        sb.Append((char)('0' + Math.Min(9, lum / 26)));
                    }
                }
                sb.Append('\n');
            }
            return sb.ToString();
        }
    }
}
'@

[WinShot]::MakeDpiAware()
$proc = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "找不到进程 $Name" }
$hwnd = [WinShot]::HandleOf([uint32]$proc.Id)
if ($hwnd -eq [IntPtr]::Zero) { throw "进程 $Name 没有可见窗口" }
Write-Output ([WinShot]::Ascii($hwnd, $Cols, $Rows, $Thresholded.IsPresent))
