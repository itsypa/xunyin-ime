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
    [switch]$Thresholded,
    # 只画一块区域（"x,y,w,h"，窗口客户区物理像素）并按 -Zoom 放大，用来"凑近看"某个控件
    [string]$Crop = "",
    [int]$Zoom = 1,
    # 列出该进程的顶层窗口（句柄 + 客户区尺寸），用来挑非主窗口（例如状态栏）
    [switch]$ListWindows,
    # 指定要画哪个窗口（按客户区宽度匹配，状态栏那种浮窗用得上）
    [int]$ClientWidth = 0
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

    public static string List(uint pid)
    {
        var sb = new StringBuilder();
        EnumWindows((h, l) =>
        {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid)
            {
                RECT r;
                GetClientRect(h, out r);
                sb.Append(h.ToString()).Append(" client=").Append(r.Right - r.Left).Append('x').Append(r.Bottom - r.Top).Append('\n');
            }
            return true;
        }, IntPtr.Zero);
        return sb.ToString();
    }

    public static IntPtr HandleWithWidth(uint pid, int width)
    {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) =>
        {
            uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid)
            {
                RECT r;
                if (GetClientRect(h, out r) && Math.Abs((r.Right - r.Left) - width) <= 4) { found = h; return false; }
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    private delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc cb, IntPtr l);

    public static string Ascii(IntPtr hwnd, int cols, int rows, bool thresholded, int cropX, int cropY, int cropW, int cropH, int zoom)
    {
        RECT r;
        if (!GetClientRect(hwnd, out r)) return "GetClientRect 失败";
        int w = r.Right - r.Left, h = r.Bottom - r.Top;
        if (w <= 0 || h <= 0) return "窗口尺寸为 0";
        if (cropW > 0 && cropH > 0) { w = cropW; h = cropH; }
        using (var bmp = new Bitmap(w, h, PixelFormat.Format32bppArgb))
        {
            using (var g = Graphics.FromImage(bmp))
            {
                IntPtr hdc = g.GetHdc();
                bool ok;
                if (cropW > 0 && cropH > 0) { ok = BitBltFromWindow(hwnd, hdc, cropX, cropY, cropW, cropH); }
                else { ok = PrintWindow(hwnd, hdc, 2); } // PW_RENDERFULLCONTENT
                g.ReleaseHdc(hdc);
                if (!ok) return "取窗口像素失败";
            }
            var sb = new StringBuilder();
            sb.Append("窗口 ").Append(w).Append('x').Append(h).Append('\n');
            for (int ry = 0; ry < rows; ry++)
            {
                for (int rx = 0; rx < cols; rx++)
                {
                    int x0 = rx * w / cols, x1 = x0 + zoom;
                    int y0 = ry * h / rows, y1 = y0 + zoom;
                    if (x1 > w) x1 = w;
                    if (y1 > h) y1 = h;
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

    [DllImport("user32.dll")] private static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] private static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll")] private static extern bool BitBlt(IntPtr d, int dx, int dy, int dw, int dh, IntPtr s, int sx, int sy, int rop);
    private static bool BitBltFromWindow(IntPtr hwnd, IntPtr hdc, int x, int y, int w, int h)
    {
        IntPtr src = GetDC(hwnd);
        if (src == IntPtr.Zero) return false;
        bool ok = BitBlt(hdc, 0, 0, w, h, src, x, y, 0x00CC0020); // SRCCOPY
        ReleaseDC(hwnd, src);
        return ok;
    }
}
'@

[WinShot]::MakeDpiAware()
$proc = Get-Process -Name $Name -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { throw "找不到进程 $Name" }
if ($ListWindows) {
    Write-Output ([WinShot]::List([uint32]$proc.Id))
    exit 0
}
$hwnd = if ($ClientWidth -gt 0) { [WinShot]::HandleWithWidth([uint32]$proc.Id, $ClientWidth) } else { [WinShot]::HandleOf([uint32]$proc.Id) }
if ($hwnd -eq [IntPtr]::Zero) { throw "进程 $Name 没有可见窗口" }
$cx = 0; $cy = 0; $cw = 0; $ch = 0
if ($Crop) {
    $p = $Crop -split ','
    if ($p.Count -ne 4) { throw "-Crop 需要 x,y,w,h" }
    $cx = [int]$p[0]; $cy = [int]$p[1]; $cw = [int]$p[2]; $ch = [int]$p[3]
}
Write-Output ([WinShot]::Ascii($hwnd, $Cols, $Rows, $Thresholded.IsPresent, $cx, $cy, $cw, $ch, [Math]::Max(1, $Zoom)))
