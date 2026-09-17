{
  cacert,
  dejavu_fonts,
  dockerTools,
  fontconfig,
  package,
}:

dockerTools.buildLayeredImage {
  name = package.pname;
  tag = "latest";
  fakeRootCommands = ''
    mkdir -p ./app ./etc ./home/nonroot
    echo 'nonroot:x:1000:1000:nonroot:/home/nonroot:/sbin/nologin' > ./etc/passwd
    echo 'nonroot:x:1000:' > ./etc/group
    chown -R 1000:1000 ./app ./home/nonroot
  '';
  contents = [
    package
    cacert
    dejavu_fonts
    fontconfig
  ];
  config = {
    Entrypoint = [ "${package}/bin/ringring-rs" ];
    Env = [ "HOME=/home/nonroot" ];
    WorkingDir = "/app";
    User = "1000:1000";
  };
}
