{
  cacert,
  dejavu_fonts,
  dockerTools,
  package,
}:

dockerTools.buildLayeredImage {
  name = package.pname;
  tag = "latest";
  fakeRootCommands = ''
    mkdir -p ./app ./etc ./home/nonroot
    echo 'nonroot:x:1000:1000:nonroot:/home/nonroot:/sbin/nologin' > ./etc/passwd
    echo 'nonroot:x:1000:' > ./etc/group
    mkdir -p ./etc/fonts
    cat > ./etc/fonts/fonts.conf <<'EOF'
    <?xml version="1.0"?>
    <!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
    <fontconfig>
      <dir>/share/fonts</dir>
    </fontconfig>
    EOF
    chown -R 1000:1000 ./app ./home/nonroot
  '';
  contents = [
    package
    cacert
    dejavu_fonts
  ];
  config = {
    Entrypoint = [ "${package}/bin/ringring-rs" ];
    Env = [ "HOME=/home/nonroot" ];
    WorkingDir = "/app";
    User = "1000:1000";
  };
}
