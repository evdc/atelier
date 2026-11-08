# Render the Atelier emblem as a 1024px PNG: a sun rising behind the horizon,
# its rays engraved as hairlines, on parchment, inside a macOS-style squircle.
import math, struct, sys, zlib
N = 1024
S = 2  # supersampling per axis
paper=(0xef,0xea,0xdf); ink=(0x1f,0x1b,0x16); glow=(0xfb,0xf8,0xf0); gold=(0x98,0x72,0x2a)
M = 100; R = 185             # icon body margin and corner radius
cx, hy = N/2, 690            # sun centre on the horizon line
sun_r = 190
def body_depth(x,y):
    """Signed distance inside the rounded rect (negative = outside)."""
    x0,y0,x1,y1 = M,M,N-M,N-M
    qx = abs(x-N/2)-(N/2-M-R); qy = abs(y-N/2)-(N/2-M-R)
    outside = math.hypot(max(qx,0),max(qy,0)) + min(max(qx,qy),0)
    return R-outside
def shade(x,y):
    edge = body_depth(x,y)
    if edge < 0: return None
    if edge < 10: return ink
    d = math.hypot(x-cx, y-hy)
    if y <= hy:
        if d <= sun_r:
            return ink if d > sun_r-9 else glow
        # rays: thin wedges radiating from the sun centre
        a = math.degrees(math.atan2(hy-y, x-cx))  # 0..180
        w = (a % 7.5)
        if d > sun_r+28 and w < 0.75 and 3 < a < 177: return ink
        return paper
    # below the horizon: horizon rule, then hatched ground
    if y < hy+9: return ink
    if ((x + y) % 26) < 3 and y > hy+30: return gold
    return paper
rows=[]
for py in range(N):
    row=bytearray([0])
    for px in range(N):
        acc=[0,0,0,0]
        for sy in range(S):
            for sx in range(S):
                c=shade(px+(sx+.5)/S, py+(sy+.5)/S)
                if c: acc[0]+=c[0];acc[1]+=c[1];acc[2]+=c[2];acc[3]+=255
        n=S*S; a=acc[3]//n
        if a: row+=bytes([acc[0]*255//acc[3],acc[1]*255//acc[3],acc[2]*255//acc[3],a])
        else: row+=b'\0\0\0\0'
    rows.append(bytes(row))
raw=b''.join(rows)
def chunk(t,d): return struct.pack('>I',len(d))+t+d+struct.pack('>I',zlib.crc32(t+d)&0xffffffff)
png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',N,N,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(raw,9))+chunk(b'IEND',b'')
open(sys.argv[1],'wb').write(png)
