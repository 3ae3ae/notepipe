"""Render the layered icon with Apple's ictool and update platform package assets.

Intermediate renders and the preview board go to exports/, which is not tracked.

Requires macOS, Xcode with Icon Composer, and Pillow.
"""
from io import BytesIO
from pathlib import Path
import subprocess
import tempfile

from PIL import Image, ImageDraw, ImageFont, ImageCms

ROOT = Path(__file__).resolve().parent
EXPORTS = ROOT / 'exports'
ICON = ROOT / 'Course2MD.icon'
RENDITIONS = {
    'Light': 'Default',
    'Dark': 'Dark',
    'Clear-Light': 'ClearLight',
    'Clear-Dark': 'ClearDark',
}


def main():
    EXPORTS.mkdir(exist_ok=True)
    developer = Path(subprocess.check_output(['xcode-select', '-p'], text=True).strip())
    candidates = (
        developer.parent / 'Applications/Icon Composer.app/Contents/Executables/ictool',
        Path('/Applications/Icon Composer.app/Contents/Executables/ictool'),
    )
    ictool = next((p for p in candidates if p.is_file()), None)
    if ictool is None:
        raise SystemExit('Icon Composer with its image-export ictool is required.')
    for label, rendition in RENDITIONS.items():
        subprocess.run([
            ictool, str(ICON), '--export-image', '--output-file',
            str(EXPORTS / f'Course2MD-{label}.png'), '--platform', 'macOS',
            '--rendition', rendition, '--width', '1024', '--height', '1024',
            '--scale', '1', '--design-generation', '27',
        ], check=True, capture_output=True)

    srgb = ImageCms.ImageCmsProfile(ImageCms.createProfile('sRGB')).tobytes()
    def normalized(path):
        im = Image.open(path)
        if im.info.get('icc_profile'):
            im = ImageCms.profileToProfile(
                im, ImageCms.ImageCmsProfile(BytesIO(im.info['icc_profile'])),
                ImageCms.createProfile('sRGB'), outputMode='RGBA')
        return im.convert('RGBA')

    assets = ROOT.parent
    light = normalized(EXPORTS / 'Course2MD-Light.png')
    dark = normalized(EXPORTS / 'Course2MD-Dark.png')
    def legacy_icon(im, size):
        # Optical inset is a design choice for the flattened, legacy ICNS.
        # Keep the editable .icon full bleed; the OS handles its enclosure.
        artwork = max(1, round(size * 824 / 1024))
        canvas = Image.new('RGBA', (size, size))
        canvas.alpha_composite(im.resize((artwork, artwork), Image.Resampling.LANCZOS),
                               ((size - artwork) // 2, (size - artwork) // 2))
        return canvas

    with tempfile.TemporaryDirectory(prefix='course2md-icon-') as td:
        iconset = Path(td) / 'Course2MD.iconset'
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                suffix = '@2x' if scale == 2 else ''
                legacy_icon(light, size * scale).save(
                    iconset / f'icon_{size}x{size}{suffix}.png', icc_profile=srgb)
        subprocess.run(['iconutil', '--convert', 'icns', '--output',
                        str(assets / 'icon.icns'), str(iconset)], check=True)
    legacy_icon(light, 1024).save(EXPORTS / 'Course2MD-macOS.png', icc_profile=srgb)

    # Windows reads resource 1 embedded from this multi-resolution ICO.
    # Linux uses the same artwork as a freestanding PNG.
    light.save(assets / 'icon.png', icc_profile=srgb)
    light.save(assets / 'icon.ico', sizes=[(s, s) for s in (16, 20, 24, 32, 40, 48, 64, 96, 128, 256)])
    for name, size in (('32x32.png', 32), ('128x128.png', 128), ('128x128@2x.png', 256)):
        light.resize((size, size), Image.Resampling.LANCZOS).save(assets / name, icc_profile=srgb)

    board = Image.new('RGBA', (1200, 820), '#F4F6FA')
    d = ImageDraw.Draw(board)
    d.rectangle((600, 0, 1200, 820), fill='#17191F')
    font_path = '/System/Library/Fonts/Supplemental/Arial.ttf'
    def font(size):
        return ImageFont.truetype(font_path, size)
    d.text((56, 45), 'course2md', font=font(42), fill='#20232C')
    d.text((57, 101), 'Video to Markdown', font=font(20), fill='#777D8C')
    d.text((656, 54), 'Icon Composer', font=font(26), fill='#F0F2F8')
    d.text((656, 98), 'One symbol. Every appearance.', font=font(18), fill='#999FAC')
    for i, (im, label, color) in enumerate(((light, 'Default', '#333A49'),
                                           (dark, 'Dark', '#DEE3EE'))):
        base = i * 600
        board.alpha_composite(im.resize((400, 400), Image.Resampling.LANCZOS), (base+100, 170))
        d.text((base+300, 596), label, font=font(19), fill=color, anchor='mm')
        for x, size in zip((150, 238, 334, 446), (16, 32, 64, 128)):
            board.alpha_composite(legacy_icon(im, size), (base+x-size//2, 695-size//2))
            d.text((base+x, 784), str(size), font=font(13), fill=color, anchor='mm')
    board.convert('RGB').save(EXPORTS / 'Course2MD-Preview.png', icc_profile=srgb)
    print('Exported native appearances and preview; updated macOS, Windows, and Linux package icons.')


if __name__ == '__main__':
    main()
