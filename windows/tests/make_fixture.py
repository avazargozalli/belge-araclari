"""Builds tests/fixtures/ornek-dilekce.udf: a synthetic UYAP document (no real data) that exercises
letterhead, header/footer, page numbers, tabs with hanging indent, a table, a numbered list,
an inline image, colours and a two-page flow. Run: python3 make_fixture.py"""
import base64, struct, zipfile, zlib
from pathlib import Path

def png(w, h, rgba):
    rows = []
    for y in range(h):
        row = bytearray(b'\x00')
        for x in range(w):
            row += bytes(rgba(x, y))
        rows.append(bytes(row))
    def chunk(t, d):
        return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(b''.join(rows), 9)) + chunk(b'IEND', b'')

# Letterhead: a band at the top and bottom, transparent elsewhere.
letterhead = png(200, 283, lambda x, y: (20, 90, 160, 255) if y < 18 or y > 262 else (0, 0, 0, 0))
badge = png(24, 24, lambda x, y: (200, 40, 40, 255) if (x - 12) ** 2 + (y - 12) ** 2 < 120 else (0, 0, 0, 0))

parts, elements = [], []
def add(text):
    start = sum(len(p.encode('utf-16-le')) // 2 for p in parts)
    parts.append(text)
    return start, len(text.encode('utf-16-le')) // 2

def run(text, **a):
    s, l = add(text)
    attrs = ' '.join(f'{k}="{v}"' for k, v in a.items())
    return f'<content {attrs} startOffset="{s}" length="{l}" />'

def para(runs, **a):
    attrs = ' '.join(f'{k}="{v}"' for k, v in a.items())
    return f'<paragraph {attrs}>{"".join(runs)}</paragraph>'

header = ''.join(para([run('\n')]) for _ in range(3))
body = [
    para([run('ÖRNEK ASLİYE HUKUK MAHKEMESİNE\n', bold='true')], Alignment='1'),
    para([run('\n')]),
    para([run('DAVACI\t', bold='true', underline='true'), run(': Örnek Kişi\n')], Alignment='0', TabSet='90.0:0:0', Hanging='96.0'),
    para([run('VEKİLİ\t', bold='true', underline='true'), run(': Av. Deneme Avukat, uzun bir adres satırı ile birlikte alt satıra kayması gereken bilgiler burada yer alır.\n')], Alignment='0', TabSet='90.0:0:0', Hanging='96.0'),
    para([run('\n')]),
    para([run('AÇIKLAMALAR\n', bold='true', underline='true')], Alignment='1'),
]
for i in range(1, 4):
    body.append(para([run(f'Talep {i}\n')], Numbered='true', NumberType='NUMBER_TYPE_NUMBER_DOT', ListId='1', ListLevel='1', LeftIndent='18.0', Hanging='18.0'))
    if i == 2:
        # Level 2 without SecListTypeLevel2 -> editor default "a."; four items reach "ç".
        for j in range(4):
            body.append(para([run(f'Alt madde {j + 1}\n')], Numbered='true', NumberType='NUMBER_TYPE_NUMBER_DOT', ListId='1', ListLevel='2', LeftIndent='36.0', Hanging='18.0'))
        body.append(para([run('Roma rakamlı alt madde\n')], Numbered='true', ListId='1', ListLevel='3', SecListTypeLevel3='NUMBER_TYPE_ROMAN_BIG_DOT', LeftIndent='54.0', Hanging='18.0'))
body.append(para([run('Beşten devam eden madde\n')], Numbered='true', NumberType='NUMBER_TYPE_NUMBER_TRE', ListId='2', ListLevel='1', NumberSetted='4', LeftIndent='18.0', Hanging='18.0'))
body.append(para([run('Madde işaretli satır\n')], Bulleted='true', BulletType='BULLET_TYPE_ELLIPSE', ListId='3', ListLevel='1', LeftIndent='18.0', Hanging='18.0'))
body.append(para([run('İçindekiler\t'), run('7\n')], TabSet='300.0:1:1'))
body.append(para([run('Vurgulu satır', background='-256'), run(' ve üstü çizili ', strikethrough='true'), run('metin.\n')]))
lorem = ('Bu paragraf, UDF dönüştürücüsünün iki yana yaslanmış metni, satır aralığını ve sayfa geçişlerini '
         'doğru işlediğini sınamak için yazılmış örnek bir metindir. Türkçe karakterler: ğüşıöç ĞÜŞİÖÇ. ')
for i in range(12):
    body.append(para([run(lorem * 2 + '\n')], Alignment='3', FirstLineIndent='35.0', LineSpacing='0.15', SpaceBelow='6.0'))
body.append(para([run('Kırmızı uyarı satırı.\n', foreground='-4194304', italic='true')], Alignment='0'))
inner = '<table border="borderCell"><row><cell>' + para([run('İç A\n')]) + '</cell><cell>' + para([run('İç B\n')]) + '</cell></row></table>'
table = ('<table columnSpans="1,1,2" border="borderRow"><row rowType="dataRow">' + ''.join(
    f'<cell>{para([run(t)])}</cell>' for t in ['Sıra\n', 'Belge\n', 'Açıklama\n']) + '</row>'
    '<row rowType="dataRow">' + ''.join(f'<cell>{para([run(t)])}</cell>' for t in ['1\n', 'Ek-1\n', 'Tablo hücresi metni\n']) + '</row>'
    '<row rowType="dataRow" columnSpans="1,3"><cell>' + para([run('2\n')]) + '</cell><cell>' + para([run('Birleştirilmiş hücre\n')]) + inner + '</cell></row></table>')
body.append(table)
s, l = add('¸')
body.append(f'<paragraph Alignment="2"><image width="24.0" height="24.0" imageData="{base64.b64encode(badge).decode()}" startOffset="{s}" length="{l}" />{run(chr(10))}</paragraph>')
body.append(para([run('Av. Deneme Avukat\n')], Alignment='2'))
footer = para([run('\n')]) + para([run('\n')])

xml = f'''<?xml version="1.0" encoding="UTF-8" ?>
<template format_id="1.8" >
<content><![CDATA[{''.join(parts)}]]></content><properties><pageFormat mediaSizeName="1" leftMargin="42.5" rightMargin="42.5" topMargin="42.5" bottomMargin="42.5" paperOrientation="1" headerFOffset="20.0" footerFOffset="20.0" /><bgImage bgImageSource="" bgImageData="{base64.b64encode(letterhead).decode()}" bgImageBottomMargin="0" bgImageUpMargin="0" bgImageRigtMargin="0" bgImageLeftMargin="0" /></properties>
<elements resolver="hvl-default" >
<header>{header}</header>{''.join(body)}<footer pageNumber-spec="BSP32_2088" pageNumber-afterStr=" ." pageNumber-seperator="/" pageNumber-fontFace="Arial" pageNumber-fontSize="10" pageNumber-color="-16777216" pageNumber-foreStr="Sayfa " pageNumber-pageStartNumStr="">{footer}</footer>
</elements>
<styles><style name="default" description="Geçerli" family="Lucida Grande" size="13" bold="false" italic="false" foreground="-16777216" /><style name="hvl-default" family="Times New Roman" size="12" description="Gövde" /></styles>
</template>'''

out = Path(__file__).parent / 'fixtures' / 'ornek-dilekce.udf'
with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as z:
    z.writestr('content.xml', xml)
print(out, out.stat().st_size, 'bytes')
