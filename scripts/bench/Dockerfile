# syntax=docker/dockerfile:1
# The benchmark image: a pixtega image plus the benchmark corpus and
# scripts/bench/bench.py. scripts/bench/bench.sh builds and runs it.
#
#   docker build -f scripts/bench/Dockerfile --build-arg IMAGE=pixtega \
#       -t pixtega-bench scripts/bench
#
# The corpus is built here, not on the machine running the benchmark, so
# that it does not depend on which vips and fonts that machine has: the
# recipe (gen-corpus.sh) runs with one Ubuntu release's vips CLI over
# photographs pinned by checksum. bench.py records a digest of the corpus
# in every result and refuses to compare results from different ones.
ARG IMAGE=pixtega-acceptance:local

FROM ubuntu:26.04 AS corpus

RUN apt-get update && apt-get install -y --no-install-recommends \
        fonts-dejavu-core \
        libheif-plugin-aomdec \
        libheif-plugin-aomenc \
        libvips-tools \
    && rm -rf /var/lib/apt/lists/*

# The Kodak lossless true color image suite: 24 photographs Kodak released
# for unrestricted use, and the customary test set for image codecs.
ADD --checksum=sha256:a56e27cbf5f843c048b6af1d6e090760e9c92fadba88b7dee0205918a37523bd \
    https://r0k.us/graphics/kodak/kodak/kodim01.png /kodak/kodim01.png
ADD --checksum=sha256:4f4b74a79237e311d72cad958237b5f7088d8bce1c82305ebefe1a70e3022dfd \
    https://r0k.us/graphics/kodak/kodak/kodim02.png /kodak/kodim02.png
ADD --checksum=sha256:e25ca1ff2f0c0cb5fdfd5f9b0a0bb21ac4c3de3c84a67f35b09a85d3306249db \
    https://r0k.us/graphics/kodak/kodak/kodim03.png /kodak/kodim03.png
ADD --checksum=sha256:e3b946107c5d3441c022f678d0c3caf1e224d81b1604ba840a4f88e562de61aa \
    https://r0k.us/graphics/kodak/kodak/kodim04.png /kodak/kodim04.png
ADD --checksum=sha256:10349e963c5c813d327852f82c1795fa4148d69fedffc4c589bee458e3ac3d53 \
    https://r0k.us/graphics/kodak/kodak/kodim05.png /kodak/kodim05.png
ADD --checksum=sha256:363510303b715d4cbc384e1ce227e466b613a09e1b71ae985882bf8e7fbd9b18 \
    https://r0k.us/graphics/kodak/kodak/kodim06.png /kodak/kodim06.png
ADD --checksum=sha256:b77d3f006f42414bb242222e0482e750c0fb9e5ee8d4bed2f6f11c5605fe54a4 \
    https://r0k.us/graphics/kodak/kodak/kodim07.png /kodak/kodim07.png
ADD --checksum=sha256:ba23983c76b4832ee0e8af0592664756841a16779acd69f792e268fb6d13d6e7 \
    https://r0k.us/graphics/kodak/kodak/kodim08.png /kodak/kodim08.png
ADD --checksum=sha256:6a4361c2fc194feb4edaa9f9a4a0620fb9943e460ac7fdf037fb0f6dd6607a7d \
    https://r0k.us/graphics/kodak/kodak/kodim09.png /kodak/kodim09.png
ADD --checksum=sha256:9dfb70f5867c29ff9ed6313683f19b3d867849e40fbc0c4c54a4a89df341cf23 \
    https://r0k.us/graphics/kodak/kodak/kodim10.png /kodak/kodim10.png
ADD --checksum=sha256:7936814b58b5387fce2e4e2488b4ec830dadd95fa9520f358ddb30990b50f2b6 \
    https://r0k.us/graphics/kodak/kodak/kodim11.png /kodak/kodim11.png
ADD --checksum=sha256:d78c37c2f04f23761ed2367dd77e2db584ddd4c3950833fecf89f199a8126980 \
    https://r0k.us/graphics/kodak/kodak/kodim12.png /kodak/kodim12.png
ADD --checksum=sha256:bc34a3ce58dea09dce1704c997171602de90cb34d0c8503a988b77f473d39b08 \
    https://r0k.us/graphics/kodak/kodak/kodim13.png /kodak/kodim13.png
ADD --checksum=sha256:55a94550ff18f3246c4074fd32b77b0c74447c26b6ad274d564d999c0450ba6e \
    https://r0k.us/graphics/kodak/kodak/kodim14.png /kodak/kodim14.png
ADD --checksum=sha256:7538cbb80cb9103606c48b806eae57d56c885c7f90b9b3be70a41160f9cbb683 \
    https://r0k.us/graphics/kodak/kodak/kodim15.png /kodak/kodim15.png
ADD --checksum=sha256:a89c7268ccd4718ba424a99fc4643c572cf692ca6eae887185ceb4e9f11d2e54 \
    https://r0k.us/graphics/kodak/kodak/kodim16.png /kodak/kodim16.png
ADD --checksum=sha256:37afcc89fbdcb76d9518e04b2fc011027e2f4cd14b3b2f83cefd721641a47c5b \
    https://r0k.us/graphics/kodak/kodak/kodim17.png /kodak/kodim17.png
ADD --checksum=sha256:1a9258c365988961d87a0598725b609139c303ad48a5aad6c503c3b1a87849aa \
    https://r0k.us/graphics/kodak/kodak/kodim18.png /kodak/kodim18.png
ADD --checksum=sha256:b7450b264b1b0a411390d8931b112c27905a992520fc90569dc4b920aa32bbdc \
    https://r0k.us/graphics/kodak/kodak/kodim19.png /kodak/kodim19.png
ADD --checksum=sha256:3b46c71e3b92a563820ba32936be8330c586c41f938efd94be938386aae4328a \
    https://r0k.us/graphics/kodak/kodak/kodim20.png /kodak/kodim20.png
ADD --checksum=sha256:ac958597c82073f6bb65129c68f72b651db5b9efd82e11547d07350214bc268b \
    https://r0k.us/graphics/kodak/kodak/kodim21.png /kodak/kodim21.png
ADD --checksum=sha256:1cee58eb1f2d9c7ebb254d208a03c783ce6cf2c4d8c2cf45e235dd23b4ce1b29 \
    https://r0k.us/graphics/kodak/kodak/kodim22.png /kodak/kodim22.png
ADD --checksum=sha256:e3111a2fd4da24af15d6459ef9eacfe54106b38e27b4a21821b75c3f5d2d5baf \
    https://r0k.us/graphics/kodak/kodak/kodim23.png /kodak/kodim23.png
ADD --checksum=sha256:1071c68372cc5a01435c2c225a5cf7d4bb803846ec08bb6b3d6721b156d7cb96 \
    https://r0k.us/graphics/kodak/kodak/kodim24.png /kodak/kodim24.png

COPY gen-corpus.sh /usr/local/bin/gen-corpus.sh
RUN gen-corpus.sh /kodak /corpus

FROM ${IMAGE}

# Root, to read the service's /proc entries whatever user the image
# declares. python3 brings nothing the service links against.
USER root
RUN apt-get update && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=corpus /corpus /corpus
COPY bench.py /usr/local/bin/bench.py
ENTRYPOINT ["python3", "/usr/local/bin/bench.py"]
CMD ["run"]
