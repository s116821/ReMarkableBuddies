Original owned protocol-v1 vectors copied from SDK commit
`9880766f5bb0bdb91445716132703b02587d372e` tests/fixtures/resident-history.
The SDK retained these from the pre-v2 wire-r2 corpus; they were not regenerated
by changing a v2 encoder's revision.

- v1-insertion.bin: SHA256 ba17908eb61a97542847ca09df1d6854b552296d4b77f65ee8de496b3a7ffea7.
- v1-insertion-observed.bin: SHA256 d6c1ed7391a1a1c8fffb34d0d347669cc4bcaaf0b1d91db617966d443e2753f4.

All identities are synthetic. No device data or native artifacts.

Original owned protocol-v2 vectors copied unchanged from SDK commit
`80b29ebec3a46b6976e5288d2e8d65fbeccca816` tests/fixtures/resident-history.
The SDK retained the pre-upgrade wire-selection-r1 corpus; these are not v3
frames with a substituted revision.

- v2-insertion.bin: SHA256 57ff674e05d62b08a8504835616b592a75f8ef4a247d5b160246fe6a90138a68.
- v2-insertion-observed.bin: SHA256 b78d2e77ad8c84f681d6487aea19468fd9c8193af8ab0d29c23cb87b7796192f.
- v2-selection.bin: SHA256 134f640a9c84ce6436082fd151a740b0b737c5db99205d2ada3762ddda8913b2.

Original owned protocol-v3 vectors copied byte-for-byte from the retained
pre-v4 wire-capture-r2 corpus used for SDK 80b29eb. These are synthetic
frames, not current frames with substituted revisions.

- v3-insertion.bin: SHA256 a281b9d592dfd1a301f0a3540b3a9ddf3103b3dd0ab9092ec77b5f8293ddca14.
- v3-insertion-observed.bin: SHA256 8a782ad4039f908ae3ba677ecc8f81831a333a08c8dcd2b1c44a8262a15bdb44.
- v3-selection.bin: SHA256 3e12b4705c15ec2e0e6b5b2e7741cdabe08c8b63b0b2d8213f06922a06d586fe.
