# disable automatic intermediate file deletion
.SECONDARY:

.PHONY: setup
setup: out/cert.key out/cert.crt

out/cert.key:
	mkdir -p out
	certtool --generate-privkey --outfile $@ --key-type=rsa --sec-param=high --seed=0000000000000000000000000000000000000000000000000000000000000000


out/cert.crt: out/cert.key cert.cfg
	certtool --generate-self-signed --load-privkey $(word 1,$^) --outfile $@ --template $(word 2,$^)
