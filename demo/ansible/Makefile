.PHONY: deploy
deploy:
	make -j8 -C ../kevlar RELEASE=1
	cp ../kevlar/kevlar.x64.stripped.elf demo.elf
	ansible-playbook playbook.yml
