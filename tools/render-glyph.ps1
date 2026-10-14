# 把图标字体里的某个字形渲染成 ASCII 点阵，方便在终端里"看"图标长什么样。
#
# 为什么要这个：给原生设置中心挑图标时，手边没有能看图的工具；用字形宽高判断不出
# "这个码位到底画的是什么"（缺字形时只会画一个空方框）。把点阵打出来就能直接读。
#
# 用法：
#   powershell -File tools\render-glyph.ps1 -Font "Segoe MDL2 Assets" -Code 0xE765
#   powershell -File tools\render-glyph.ps1 -Font "Segoe MDL2 Assets" -Code 0xE765,0xE7C3 -Size 24
param(
    [string]$Font = "Segoe MDL2 Assets",
    # 逗号分隔的十六进制码位，例如 "E765,E7C3"
    [string]$Codes = "E765",
    [int]$Size = 26,
    [int]$Threshold = 128
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

# 渲染逻辑写成 C#：PowerShell 传 System.Drawing.Font 参数时会做一次莫名的类型转换而失败
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Text;

public static class GlyphArt
{
    public static string Render(string family, int codePoint, int size, int threshold)
    {
        var sb = new StringBuilder();
        using (var bmp = new Bitmap(size, size))
        using (var g = Graphics.FromImage(bmp))
        using (var font = new Font(family, size * 0.82f, FontStyle.Regular, GraphicsUnit.Pixel))
        {
            g.Clear(Color.Black);
            g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.AntiAlias;
            g.DrawString(char.ConvertFromUtf32(codePoint), font, Brushes.White, 0, 0);
            for (int y = 0; y < size; y++)
            {
                for (int x = 0; x < size; x++)
                {
                    int v = bmp.GetPixel(x, y).R;
                    sb.Append(v >= threshold ? '#' : (v >= threshold / 3 ? '+' : '.'));
                }
                sb.Append('\n');
            }
        }
        return sb.ToString();
    }
}
'@

$Code = @($Codes -split '[,\s]+' | Where-Object { $_ } | ForEach-Object {
        [Convert]::ToInt32($_.Trim().TrimStart("0", "x", "X"), 16)
    })

foreach ($cp in $Code) {
    Write-Output ("=== U+{0:X4}  {1}" -f $cp, $Font)
    Write-Output ([GlyphArt]::Render($Font, $cp, $Size, $Threshold))
}
