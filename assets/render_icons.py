
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parents[1]
assets = root / "assets"
master = Image.open(assets / "redline-1024.png").convert("RGBA")
fore = Image.open(assets / "redline-foreground-1024.png").convert("RGBA")

# A 256 master for Windows/window icons, and a 512 store image.
master.resize((256, 256), Image.Resampling.LANCZOS).save(assets / "redline-256.png")
master.resize((512, 512), Image.Resampling.LANCZOS).save(assets / "redline-512.png")

# Windows ICO, all sizes Explorer/taskbar may ask for.
master.save(assets / "redline.ico", format="ICO", sizes=[(16,16),(24,24),(32,32),(48,48),(64,64),(128,128),(256,256)])

res = root / "android/app/src/main/res"
legacy = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
foreground = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}
for density, size in legacy.items():
    d = res / f"mipmap-{density}"
    d.mkdir(parents=True, exist_ok=True)
    square = master.resize((size, size), Image.Resampling.LANCZOS)
    square.save(d / "ic_launcher.png")

    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, size - 1, size - 1), fill=255)
    round_icon = square.copy()
    alpha = Image.composite(square.getchannel("A"), Image.new("L", (size, size), 0), mask)
    round_icon.putalpha(alpha)
    round_icon.save(d / "ic_launcher_round.png")

    fsize = foreground[density]
    fg = fore.resize((fsize, fsize), Image.Resampling.LANCZOS)
    fg.save(d / "ic_launcher_foreground.png")

# Contact sheet on a checkerboard so the transparent corners are obvious.
check = Image.new("RGBA", (900, 320), (24, 24, 28, 255))
draw = ImageDraw.Draw(check)
for y in range(0, 320, 20):
    for x in range(0, 900, 20):
        if (x // 20 + y // 20) % 2:
            draw.rectangle((x, y, x + 19, y + 19), fill=(34, 34, 40, 255))
x = 20
for size in (16, 24, 32, 48, 64, 128, 256):
    icon = master.resize((size, size), Image.Resampling.LANCZOS)
    check.alpha_composite(icon, (x, 20 + (256 - size) // 2))
    x += size + 24
check.save(assets / "icon-sizes-preview.png")

print("ICO", (assets / "redline.ico").stat().st_size)
print("alpha corner", master.getpixel((0, 0)))
print("alpha center", master.getpixel((512, 512)))
