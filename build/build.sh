#!/bin/bash
set -x
set -e
version=1.7.6
RPM_BUILD_DIR="/root/rpmbuild"
rm -rf  $RPM_BUILD_DIR/BUILD
rm -rf  $RPM_BUILD_DIR/BUILDROOT
rm -rf  $RPM_BUILD_DIR/RPMS
rm -rf  $RPM_BUILD_DIR/SOURCES
rm -rf  $RPM_BUILD_DIR/SPECS
rm -rf  $RPM_BUILD_DIR/SRPMS
rm -rf  $RPM_BUILD_DIR/containerd.io/build/RPMS
rm -rf  $RPM_BUILD_DIR/containerd.io/build/SRPMS

mkdir -p $RPM_BUILD_DIR/SOURCES
mkdir -p $RPM_BUILD_DIR/SPECS

cp $RPM_BUILD_DIR/containerd.io/build/containerd.toml     $RPM_BUILD_DIR/SOURCES
cp $RPM_BUILD_DIR/containerd.io/containerd.service  $RPM_BUILD_DIR/SOURCES
cp $RPM_BUILD_DIR/containerd.io/build/containerd.spec  $RPM_BUILD_DIR/SPECS
cp $RPM_BUILD_DIR/containerd.io/build/containerd-kylin.spec  $RPM_BUILD_DIR/SPECS
cd $RPM_BUILD_DIR

cp -r containerd.io containerd.io-${version} 
tar -cf containerd-${version}.tar.gz  containerd.io-${version}
rm -rf   containerd.io-${version}
mv $RPM_BUILD_DIR/containerd-${version}.tar.gz   $RPM_BUILD_DIR/SOURCES

tar -cf runc.tar.gz runc
mv $RPM_BUILD_DIR/runc.tar.gz $RPM_BUILD_DIR/SOURCES

hostLinuxVersion=`uname -r`
el7="el7"
if [[ $hostLinuxVersion =~ $el7 ]]
then
  linuxVersion="el7"
else
  linuxVersion="el8"
fi

buildTime=$(date +%Y%m%d%H%M)

pushd /root/rpmbuild/containerd.io
   ref=$(git rev-parse HEAD)
   gitCommit=$(git rev-parse --short HEAD)
   gitCommitMessage="$(git log -1 --pretty='%s')"
   gchVerify="bump X from"
   gchgitVersion="git${gitCommit}"
   if [ -f "/root/rpmbuild/containerd.io/build/GCH.txt" ];then
     gchVersion=`cat /root/rpmbuild/containerd.io/build/GCH.txt`
   else
     gchVersion="t1.0"
   fi
popd
 
pushd $RPM_BUILD_DIR/runc
   runcCommit=$(git rev-parse --short HEAD)
popd

RUNC_VERSION=v1.0.3

export VERSION=$version
export REF=$ref
pushd /root/rpmbuild/containerd.io/build
  python changelog.py
popd

if [[ $(uname -r) =~ "ky" ]]; then
  sed -i 's/LimitNOFILE=infinity/LimitNOFILE=1048576/g' $RPM_BUILD_DIR/SOURCES/containerd.service
  rpmbuild -ba  ./SPECS/containerd-kylin.spec  --define "dist .${KYLINVERSION}" --define "buildtime $buildTime" \
--define "gitcommit $gitCommit" --define "runcCommit $runcCommit" --define "runcVersion $RUNC_VERSION" --define "containerdVersion $version"
elif [[ $(uname -r) =~ "el8" ]]; then
  rpmRelease="1.%{?buildtime}git%{?gitcommit}.cgsl6_2"
  rpmbuild -ba  ./SPECS/containerd.spec  --define "_release $rpmRelease"  --define "buildtime $buildTime" \
--define "gitcommit $gitCommit" --define "runcCommit $runcCommit" --define "runcVersion $RUNC_VERSION" --define "containerdVersion $version"
elif [ $ISLONGXI ];then
  sysVersion="zncgsl6"
  if [[ $gitCommitMessage =~ $gchVerify ]]
  then
    rpmRelease="1.${sysVersion}.${gchVersion}"
  else
    rpmRelease="1.${sysVersion}.${gchVersion}.${gchgitVersion}"
  fi
  rpmbuild -ba  ./SPECS/containerd.spec --nodebuginfo --define "_release $rpmRelease"  --define "buildtime $buildTime" \
--define "gitcommit $gitCommit" --define "runcCommit $runcCommit" --define "runcVersion $RUNC_VERSION" --define "containerdVersion $version"
else
  rpmRelease="1.%{?linuxVersion}.%{?buildtime}git%{?gitcommit}"
  rpmbuild -ba  ./SPECS/containerd.spec --define "_release $rpmRelease" --define "linuxVersion $linuxVersion" --define "buildtime $buildTime" \
--define "gitcommit $gitCommit" --define "runcCommit $runcCommit" --define "runcVersion $RUNC_VERSION" --define "containerdVersion $version"
fi

mv $RPM_BUILD_DIR/RPMS   $RPM_BUILD_DIR/containerd.io/build/
mv $RPM_BUILD_DIR/SRPMS   $RPM_BUILD_DIR/containerd.io/build/
