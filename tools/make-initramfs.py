#!/usr/bin/env python3
"""Create a reproducible newc initramfs that switches to the persistent RV64 root disk."""
import argparse, pathlib
p=argparse.ArgumentParser();p.add_argument('busybox');p.add_argument('output');a=p.parse_args()
data=pathlib.Path(a.busybox).read_bytes()
entries=[('dev',0o040755,b''),('dev/console',0o020600,b''),('proc',0o040755,b''),('sys',0o040755,b''),('newroot',0o040755,b''),('tmp',0o041777,b''),('root',0o040700,b''),('bin',0o040755,b''),('bin/busybox',0o100755,data),('init',0o100755,b'''#!/bin/busybox sh
/bin/busybox --install -s /bin
export PATH=/bin:/sbin
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
echo WASMNUX_MOUNTING_ROOT
mount -t ext2 /dev/vda /newroot || { echo ROOT_MOUNT_FAILED; exec /bin/sh; }
echo WASMNUX_SWITCHING_ROOT
mount --move /dev /newroot/dev
exec switch_root /newroot /sbin/init
echo SWITCH_ROOT_FAILED
exec /bin/sh
''')]
out=bytearray();ino=1
for name,mode,body in entries:
 name=name.encode();header=b'070701'+b''.join(f'{x:08X}'.encode() for x in [ino,mode,0,0,1,0,len(body),0,0,0,0,len(name)+1,0]);ino+=1
 out+=header+name+b'\0';out+=b'\0'*((-len(out))%4);out+=body;out+=b'\0'*((-len(out))%4)
name=b'TRAILER!!!';out+=b'070701'+b''.join(f'{x:08X}'.encode() for x in [0,0,0,0,1,0,0,0,0,0,0,len(name)+1,0])+name+b'\0';out+=b'\0'*((-len(out))%512)
pathlib.Path(a.output).write_bytes(out)
