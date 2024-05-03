VERSION := 0.1.0

.PHONY: clean
clean:
	@rm -vf docker/sourceforge-dl

.PHONY: build.bin
build.bin:
	cargo build -r

.PHONY: build.image
build.image: build.bin
	cp target/release/sourceforge-dl docker/sourceforge-dl
	cd docker && docker buildx build --no-cache -t jdxj/sourceforge-dl:$(VERSION) .

.PHONY: push.image
push.image: build.image
	docker push jdxj/sourceforge-dl:$(VERSION)
