//! Bit-exact port of MSVCR110's SSE2 `asin`: `msvcr110.dll` export `_libm_sse2_asin_precise`
//! (RVA 0x34e40), which Server.exe imports (thunk `0x0054a9bc`) and calls only through the
//! single-precision wrapper at `Server.exe 0x00402480` (`(float)asin((double)x)`), itself
//! called from `turnTowards` 0x005306d0 and `cube::World::tick` 0x005322d0.
//!
//! Transcribed from the disassembly (capstone over `game/msvcr110.dll`), one statement per
//! SSE2 instruction in the original order; `_lo` / `_hi` name the lanes of a packed register.
//! Checked against `tests/golden/asin_msvcr110.txt` (`tools/oracle/libm_emu.py`).
//!
//! # Paths (by `k = (bits >> 44) & 0x7ffff`: exponent and top 8 fraction bits of `|x|`)
//!
//! - `0.0625 <= |x| < 0.8652`: [`mid`], a table of `asin(c)` (double-double) and
//!   `sqrt(1 - c^2)` at 240 points `c`, and `asin(x) = asin(c) + asin(d)` with
//!   `d = x sqrt(1-c^2) - c sqrt(1-x^2)` evaluated in the stable form
//!   `(x^2 - c^2) / (x sqrt(1-c^2) + c sqrt(1-x^2))`.
//! - `0.8652 <= |x| < 0.9961`: [`upper`], the same identity applied to
//!   `asin(|x|) = pi/2 - asin(sqrt(1 - x^2))`.
//! - `2^-60 <= |x| < 0.0625`: [`small`], an odd polynomial.
//! - `0.9961 <= |x| < 1`: [`near_one`], `pi/2 - asin(s)` with `s = sqrt(1 - x^2)` in
//!   extra precision and an odd polynomial in `s`.
//! - otherwise [`special`]: `|x| < 2^-60` returns `x`; `|x| == 1` returns `±pi/2`; a NaN
//!   returns itself quieted; `|x| > 1` returns the default NaN (`0xfff8000000000000`). The
//!   last two go through the math error handler, which returns the value unchanged.

/// `sqrt(1 - c^2)` for the 256 table points (RVA 0x36290). [`mid`] uses the first 240.
#[rustfmt::skip]
pub(crate) const SQRT_TABLE: [u64; 256] = [
    0x3fefefbb9d85b0fd, 0x3fefef395a448f23, 0x3fefeeb513dad6c5, 0x3fefee2eca2f8598, // 0
    0x3fefeda67d29363e, 0x3fefed1c2cae202c, 0x3fefec8fd8a41794, 0x3fefec0180f08d4d, // 4
    0x3fefeb7125788eb6, 0x3fefeadec620c59f, 0x3fefea4a62cd782f, 0x3fefe9b3fb6288c8, // 8
    0x3fefe91b8fc375ed, 0x3fefe8811fd35a28, 0x3fefe7e4ab74ebec, 0x3fefe746328a7d7b, // 12
    0x3fefe6a5b4f5fcc9, 0x3fefe6033298f35f, 0x3fefe55eab54863e, 0x3fefe4b81f0975c2, // 16
    0x3fefe40f8d981d81, 0x3fefe364f6e07434, 0x3fefe2b85ac20b8e, 0x3fefe209b91c1028, // 20
    0x3fefe15911cd4957, 0x3fefe0a664b41914, 0x3fefdff1b1ae7bd8, 0x3fefdf3af89a087c, // 24
    0x3fefde823953f017, 0x3fefddc773b8fdde, 0x3fefdd0aa7a59702, 0x3fefdc4bd4f5ba8d, // 28
    0x3fefdb8afb85013f, 0x3fefdac81b2e9d6c, 0x3fefda0333cd5ad7, 0x3fefd93c453b9e8f, // 32
    0x3fefd8734f5366ca, 0x3fefd7a851ee4ac4, 0x3fefd6db4ce57a92, 0x3fefd60c4011bf04, // 36
    0x3fefd53b2b4b797b, 0x3fefd4680e6aa3c3, 0x3fefd392e946cfef, 0x3fefd2bbbbb7282d, // 40
    0x3fefd1e285926ea2, 0x3fefd10746aefd40, 0x3fefd029fee2c59f, 0x3fefcf4aae0350d3, // 44
    0x3fefce6953e5bf43, 0x3fefcd85f05ec87f, 0x3fefcca08342bb15, 0x3fefcbb90c657c69, // 48
    0x3fefcacf8b9a8886, 0x3fefc9e400b4f1f7, 0x3fefc8f66b876195, 0x3fefc806cbe41663, // 52
    0x3fefc715219ce558, 0x3fefc6216c833938, 0x3fefc52bac681266, 0x3fefc433e11c06b0, // 56
    0x3fefc33a0a6f4129, 0x3fefc23e283181f3, 0x3fefc1403a321e12, 0x3fefc040403fff3e, // 60
    0x3fefbebc72814922, 0x3fefbcb0348a8536, 0x3fefba9bc34003f7, 0x3fefb87f1d04d3a4, // 64
    0x3fefb65a40353637, 0x3fefb42d2b269ae4, 0x3fefb1f7dc279777, 0x3fefafba517fe194, // 68
    0x3fefad74897047dc, 0x3fefab268232aaf1, 0x3fefa8d039f9f658, 0x3fefa671aef21942, // 72
    0x3fefa40adf3fff2f, 0x3fefa19bc9018876, 0x3fef9f246a4d82ac, 0x3fef9ca4c133a0e7, // 76
    0x3fef9a1ccbbc73e4, 0x3fef978c87e9620c, 0x3fef94f3f3b49f56, 0x3fef92530d112508, // 80
    0x3fef8fa9d1eaa956, 0x3fef8cf8402596df, 0x3fef8a3e559f0408, 0x3fef877c102caa32, // 84
    0x3fef84b16d9cdcd0, 0x3fef81de6bb68056, 0x3fef7f0308390106, 0x3fef7c1f40dc4998, // 88
    0x3fef79331350b9c1, 0x3fef763e7d3f1c8c, 0x3fef73417c489e96, 0x3fef703c0e06c423, // 92
    0x3fef6d2e300b5f08, 0x3fef6a17dfe08474, 0x3fef66f91b08828f, 0x3fef63d1defdd5ef, // 96
    0x3fef60a229331eed, 0x3fef5d69f71316cc, 0x3fef5a29460084b3, 0x3fef56e01356328b, // 100
    0x3fef538e5c66e1a6, 0x3fef50341e7d3f42, 0x3fef4cd156dbd8e2, 0x3fef496602bd107b, // 104
    0x3fef45f21f531075, 0x3fef4275a9c7bf7c, 0x3fef3ef09f3cb431, 0x3fef3b62fccb289d, // 108
    0x3fef37ccbf83ed86, 0x3fef342de46f5d88, 0x3fef3086688d500e, 0x3fef2cd648d50c0c, // 112
    0x3fef291d82353a99, 0x3fef255c1193d949, 0x3fef2191f3ce2c66, 0x3fef1dbf25b8b0ea, // 116
    0x3fef19e3a41f0e4e, 0x3fef15ff6bc40824, 0x3fef121279616f80, 0x3fef0e1cc9a8142a, // 120
    0x3fef0a1e593fb59c, 0x3fef061724c6f3cd, 0x3fef020728d33fc2, 0x3feefdee61f0cbee, // 124
    0x3feef7b8b37939f8, 0x3feeef52390d1539, 0x3feee6c852e79225, 0x3feede1ae3a3af46, // 128
    0x3feed549cd40b9fc, 0x3feecc54f120186a, 0x3feec33c300303b3, 0x3feeb9ff6a08321f, // 132
    0x3feeb09e7ea970c1, 0x3feea7194cb92c17, 0x3fee9d6fb25fe740, 0x3fee93a18d19a137, // 136
    0x3fee89aeb9b3279f, 0x3fee7f9714475693, 0x3fee755a783c44e7, 0x3fee6af8c0405c60, // 140
    0x3fee6071c6475d29, 0x3fee55c563874c0b, 0x3fee4af370754aae, 0x3fee3ffbc4c25943, // 144
    0x3fee34de375800eb, 0x3fee299a9e54e61b, 0x3fee1e30cf09424b, 0x3fee12a09df34426, // 148
    0x3fee06e9debb556c, 0x3fedfb0c643045b7, 0x3fedef080043594b, 0x3fede2dc84043af9, // 152
    0x3fedd689bf9cd040, 0x3fedca0f824cee9f, 0x3fedbd6d9a65f123, 0x3fedb0a3d5462d1f, // 156
    0x3feda3b1ff5444f3, 0x3fed9697e3fa57ce, 0x3fed89554da10d2a, 0x3fed7bea05aa7acc, // 160
    0x3fed6e55d46ce40f, 0x3fed6098812d510a, 0x3fed52b1d219fc3e, 0x3fed44a18c449556, // 164
    0x3fed3667739c576f, 0x3fed28034ae7f155, 0x3fed1974d3bf3e18, 0x3fed0abbce84cc32, // 168
    0x3fecfbd7fa5f3181, 0x3fececc915322a24, 0x3fecdd8edb97805b, 0x3fecce2908d7bb4e, // 172
    0x3fecbe9756e2929a, 0x3fecaed97e47246c, 0x3fec9eef362bebd9, 0x3fec8ed8344674f1, // 176
    0x3fec7e942cd2cc10, 0x3fec6e22d28aa5b5, 0x3fec5d83d69c3c15, 0x3fec4cb6e8a0df7c, // 180
    0x3fec3bbbb693366c, 0x3fec2a91ecc52a36, 0x3fec193935d57cbd, 0x3fec07b13aa503ca, // 184
    0x3febf5f9a24b8648, 0x3febe412120c3773, 0x3febd1fa2d49cbf1, 0x3febbfb1957a2482, // 188
    0x3feba3e88e5c39ed, 0x3feb7e2e7c3ed68e, 0x3feb57aa912de205, 0x3feb3059735c5e90, // 192
    0x3feb0837a370523a, 0x3feadf417a62a16d, 0x3feab5732734fa47, 0x3fea8ac8ac79d249, // 196
    0x3fea5f3dddaa0225, 0x3fea32ce5c4305ef, 0x3fea057594a84fc8, 0x3fe9d72ebac16dd6, // 200
    0x3fe9a7f4c64dfe16, 0x3fe977c26ee7878a, 0x3fe9469227a84b4b, 0x3fe9145e1a6cf381, // 204
    0x3fe8e12022a5ab3a, 0x3fe8acd1c7a997f1, 0x3fe8776c367dda86, 0x3fe840e83aff1d1d, // 208
    0x3fe8093e385a3700, 0x3fe7d06620bd8524, 0x3fe796576c292765, 0x3fe75b090e40447a, // 212
    0x3fe71e716af8a794, 0x3fe6e0864a0050d6, 0x3fe6a13cc8a9b946, 0x3fe660894a2751cc, // 216
    0x3fe61e5f65d4d978, 0x3fe5dab1d341202a, 0x3fe59572539c229b, 0x3fe54e91981b7778, // 220
    0x3fe505ff24d0e46b, 0x3fe4bba92f53830a, 0x3fe46f7c7879a3bd, 0x3fe4216420369e50, // 224
    0x3fe3d14972795a11, 0x3fe37f13aba2fbb6, 0x3fe32aa7b2d3fd8f, 0x3fe2d3e7c7da540a, // 228
    0x3fe27ab321f3dcf1, 0x3fe21ee57bd01fd8, 0x3fe1c0568830ae9d, 0x3fe15ed9491d3888, // 232
    0x3fe0fa3b41afe8ad, 0x3fe0924377cc95c0, 0x3fe026b137474aca, 0x3fdf6e75051127be, // 236
    0x3fde87142982b1b1, 0x3fdd96799ba6f66d, 0x3fdc9bbc9bb903fa, 0x3fdb95c8d37c8daa, // 240
    0x3fda8351e7be222c, 0x3fd962c20e982feb, 0x3fd83220fb335650, 0x3fd6eeee8953be07, // 244
    0x3fd595e8ede2700d, 0x3fd422aeba618156, 0x3fd28f1c6c540fba, 0x3fd0d21c6aeb7150, // 248
    0x3fcdba59d10bfe08, 0x3fc92ca4f0107011, 0x3fc389d6226c1299, 0x3fb69af589b35963, // 252
];

/// `[asin(c) low part, asin(c) high part]` (RVA 0x35390).
#[rustfmt::skip]
pub(crate) const ASIN_TABLE: [[u64; 2]; 240] = [
    [0x3ce4facce3dda158, 0x3fb022bc0ae53100], // 0
    [0x3cc53bfc86f52717, 0x3fb062dd26afc300], // 1
    [0x3ceefc676b1e2c99, 0x3fb0a2ff4a182100], // 2
    [0x3ce1ee31546582c4, 0x3fb0e32279319d00], // 3
    [0x3ce717fc8c7a10fa, 0x3fb12346b8101d00], // 4
    [0x3cec61c29f168d56, 0x3fb1636c0ac82400], // 5
    [0x3ce0b278d493a30d, 0x3fb1a392756ed200], // 6
    [0x3ce019eb3576d563, 0x3fb1e3b9fc19e500], // 7
    [0x3ce01a4e19fd9730, 0x3fb223e2a2dfbe00], // 8
    [0x3cd8240822c1c109, 0x3fb2640c6dd76200], // 9
    [0x3ce1c61651af678c, 0x3fb2a43761187c00], // 10
    [0x3cdc1bbc8fbae9e8, 0x3fb2e46380bb6100], // 11
    [0x3ce9a05f480b300d, 0x3fb32490d0d91000], // 12
    [0x3cc80f3f201d555d, 0x3fb364bf558b3800], // 13
    [0x3cd53fa3f37d7d7c, 0x3fb3a4ef12ec3500], // 14
    [0x3ce1f2dc089b2b7e, 0x3fb3e5200d171800], // 15
    [0x3ccbad42af3e3029, 0x3fb425524827a700], // 16
    [0x3cb3264981c019ab, 0x3fb46585c83a5e00], // 17
    [0x3ce8ca128eca213e, 0x3fb4a5ba916c7300], // 18
    [0x3ccf717e6263c365, 0x3fb4e5f0a7dbdb00], // 19
    [0x3cc280cadcaa9372, 0x3fb526280fa74600], // 20
    [0x3cd2e1046e9af65a, 0x3fb56660ccee2700], // 21
    [0x3cc0e26a3704d634, 0x3fb5a69ae3d0b500], // 22
    [0x3cc35509ffb3692b, 0x3fb5e6d6586fec00], // 23
    [0x3cd73b4b2eef9f46, 0x3fb627132eed9100], // 24
    [0x3ce81489c5fc6859, 0x3fb667516b6c3400], // 25
    [0x3ce7c2559e2b3bf6, 0x3fb6a791120f3300], // 26
    [0x3ce5386d099cd0dc, 0x3fb6e7d226fabb00], // 27
    [0x3cccf9ab98f38eeb, 0x3fb72814ae53cc00], // 28
    [0x3c74df3dd959141a, 0x3fb76858ac403a00], // 29
    [0x3ced6034406ee42c, 0x3fb7a89e24e6b000], // 30
    [0x3ce705f1ef1cd1db, 0x3fb7e8e51c6eb600], // 31
    [0x3cdfaedcf03994fe, 0x3fb8292d9700ad00], // 32
    [0x3cc4bfbb9daa5c07, 0x3fb8697798c5d600], // 33
    [0x3cef3d11e5e9a1e0, 0x3fb8a9c325e85200], // 34
    [0x3ca5ff40d81f66fd, 0x3fb8ea1042932a00], // 35
    [0x3ceabc77d9c9ad61, 0x3fb92a5ef2f24700], // 36
    [0x3ce6f73c93286daf, 0x3fb96aaf3b328100], // 37
    [0x3cde45a050f306a0, 0x3fb9ab011f819800], // 38
    [0x3ceadca8832fd63c, 0x3fb9eb54a40e3a00], // 39
    [0x3ca9a717be8f7446, 0x3fba2ba9cd080800], // 40
    [0x3cd8507a6269824e, 0x3fba6c009e9f9200], // 41
    [0x3ce658252020fcb0, 0x3fbaac591d066100], // 42
    [0x3c9c7d5fbaec405d, 0x3fbaecb34c6ef600], // 43
    [0x3cbc09479a9cbcfb, 0x3fbb2d0f310cca00], // 44
    [0x3cef28a17fe9dc61, 0x3fbb6d6ccf145500], // 45
    [0x3ce0913fed095469, 0x3fbbadcc2abb1100], // 46
    [0x3cb6fad72acfe356, 0x3fbbee2d48377700], // 47
    [0x3cb4465b588d16ad, 0x3fbc2e902bc10600], // 48
    [0x3ce06e6b15208e58, 0x3fbc6ef4d9904500], // 49
    [0x3ce52b8d28a954db, 0x3fbcaf5b55dec600], // 50
    [0x3ce49c06d41b89d6, 0x3fbcefc3a4e72700], // 51
    [0x3ca995b83421756a, 0x3fbd302dcae51600], // 52
    [0x3ce1efa2f7a817d6, 0x3fbd7099cc155100], // 53
    [0x3ce28abf86e5ba05, 0x3fbdb107acb5ae00], // 54
    [0x3cebdc7b98b60287, 0x3fbdf17771051800], // 55
    [0x3cc9f098e4a8575f, 0x3fbe31e91d439600], // 56
    [0x3c01d3fac6029027, 0x3fbe725cb5b24900], // 57
    [0x3c6304cc44aeedd1, 0x3fbeb2d23e937300], // 58
    [0x3cedc1d2d6abe9c7, 0x3fbef349bc2a7700], // 59
    [0x3cc424276e93a5b2, 0x3fbf33c332bbe000], // 60
    [0x3cd937fc12a2a77a, 0x3fbf743ea68d5b00], // 61
    [0x3cd7d469412d5605, 0x3fbfb4bc1be5c300], // 62
    [0x3ce1625444b26007, 0x3fbff53b970d1e00], // 63
    [0x3ce8892e889f1153, 0x3fc02aff52065400], // 64
    [0x3cd399e6c452b129, 0x3fc06b84f8e03200], // 65
    [0x3ce48bd9d825ac2c, 0x3fc0ac0ed1fe7200], // 66
    [0x3cdd0a3f14435314, 0x3fc0ec9cee9e4800], // 67
    [0x3cf6e566cc67785a, 0x3fc12d2f6006f000], // 68
    [0x3cea23f388f8f708, 0x3fc16dc63789de00], // 69
    [0x3cdf0dac810677c7, 0x3fc1ae618682e600], // 70
    [0x3ce1fec043e0225f, 0x3fc1ef015e586c00], // 71
    [0x3cf9a905409caf8e, 0x3fc22fa5d07b9000], // 72
    [0x3cf55ef21838a824, 0x3fc2704eee685d00], // 73
    [0x3cff8f2a58bc1c62, 0x3fc2b0fcc9a5f300], // 74
    [0x3cbecf5f977d1ca9, 0x3fc2f1af73c6ba00], // 75
    [0x3cf474ca7008baf7, 0x3fc33266fe688900], // 76
    [0x3cee5601b524a2b7, 0x3fc373237b34de00], // 77
    [0x3cd70a67f9c88f55, 0x3fc3b3e4fbe10500], // 78
    [0x3ceab3b1771fb799, 0x3fc3f4ab922e4a00], // 79
    [0x3ce8f60c5a19a049, 0x3fc435774fea2a00], // 80
    [0x3cf590402efd87ed, 0x3fc4764846ee8000], // 81
    [0x3ce7f386e5db094e, 0x3fc4b71e8921b800], // 82
    [0x3cf3f6b5bf6a05e3, 0x3fc4f7fa2876fc00], // 83
    [0x3ceb2216ee7a98af, 0x3fc538db36ee6900], // 84
    [0x3cfabdbd213f8900, 0x3fc579c1c6953c00], // 85
    [0x3cf0e31e6fe1ac47, 0x3fc5baade9860800], // 86
    [0x3cf5f7c8466578c3, 0x3fc5fb9fb1e8e300], // 87
    [0x3ce2da54fff947d4, 0x3fc63c9731f39d00], // 88
    [0x3cf91980da09e656, 0x3fc67d947be9ee00], // 89
    [0x3cc2ac8330e59aa5, 0x3fc6be97a21daf00], // 90
    [0x3cff834061fdcad9, 0x3fc6ffa0b6ef0500], // 91
    [0x3cc19692a5301ca6, 0x3fc740afcccca000], // 92
    [0x3cfa1d31e70d14a1, 0x3fc781c4f633e200], // 93
    [0x3ce2551a612ea884, 0x3fc7c2e045b12100], // 94
    [0x3cd2f488a895499d, 0x3fc80401cddfd100], // 95
    [0x3cd1e3b709c7d6f9, 0x3fc84529a16ac000], // 96
    [0x3cfec1acb4fe144f, 0x3fc88657d30c4900], // 97
    [0x3cf32965dd309857, 0x3fc8c78c758e8e00], // 98
    [0x3cc8540ae794a2fe, 0x3fc908c79bcba900], // 99
    [0x3cf8893fa4c1cfd5, 0x3fc94a0958ade600], // 100
    [0x3cfc39374f500721, 0x3fc98b51bf2ffe00], // 101
    [0x3cf9a458e91d3bf5, 0x3fc9cca0e25d4a00], // 102
    [0x3cf2f1d43a653a56, 0x3fca0df6d551fe00], // 103
    [0x3ccf60dca86d57ef, 0x3fca4f53ab3b6200], // 104
    [0x3cf4a62717645434, 0x3fca90b777580a00], // 105
    [0x3cfc0103de5980d0, 0x3fcad2224cf81400], // 106
    [0x3cf253a9ddf4b964, 0x3fcb13943f7d5f00], // 107
    [0x3cf478069d1854b7, 0x3fcb550d625bc600], // 108
    [0x3cf781237ae75ca3, 0x3fcb968dc9195e00], // 109
    [0x3ceabe97130ebc31, 0x3fcbd815874eb100], // 110
    [0x3cfab817856177e3, 0x3fcc19a4b0a6f900], // 111
    [0x3cfe394e1bf1eafd, 0x3fcc5b3b58e06100], // 112
    [0x3ce35d275990bd9a, 0x3fcc9cd993cc4000], // 113
    [0x3cea45ee1cc8ad1a, 0x3fccde7f754f5600], // 114
    [0x3cf45880c3975321, 0x3fcd202d11620f00], // 115
    [0x3cf6e107923ab243, 0x3fcd61e27c10c000], // 116
    [0x3cfb24b0af3cae42, 0x3fcda39fc97be700], // 117
    [0x3ce94755a9ea582b, 0x3fcde5650dd86d00], // 118
    [0x3cd7078adb06553e, 0x3fce27325d6fe500], // 119
    [0x3cead949008ea106, 0x3fce6907cca0d000], // 120
    [0x3ce1d6b310d6fd47, 0x3fceaae56fdee000], // 121
    [0x3cc16e99cedadb20, 0x3fceeccb5bb33900], // 122
    [0x3cc75ee47c8b09e9, 0x3fcf2eb9a4bcb600], // 123
    [0x3cd3ac6a1a6f3e83, 0x3fcf70b05fb02e00], // 124
    [0x3cf7c199b3f05331, 0x3fcfb2afa158b800], // 125
    [0x3cfc6cc3ea9e931c, 0x3fcff4b77e97f300], // 126
    [0x3cf715dcb2782e6f, 0x3fd01b6406332500], // 127
    [0x3d007bbd13f8869d, 0x3fd04cf8ad203400], // 128
    [0x3d06c521567fcb11, 0x3fd08f23ce016200], // 129
    [0x3d0967401a01f08d, 0x3fd0d1610f0c1e00], // 130
    [0x3d097aa4020fe547, 0x3fd113b0c65d8800], // 131
    [0x3d0d87369da09601, 0x3fd156134ada6f00], // 132
    [0x3d0562b220f38e4a, 0x3fd19888f4342700], // 133
    [0x3ce13ede7490b52f, 0x3fd1db121aed7700], // 134
    [0x3cf8d7d6cc60eb61, 0x3fd21daf185fa300], // 135
    [0x3d0765ae09dd611f, 0x3fd2606046bf9500], // 136
    [0x3d09661619fa2f12, 0x3fd2a32601231e00], // 137
    [0x3cf834546d533595, 0x3fd2e600a3865700], // 138
    [0x3cb9efaf1d7ab552, 0x3fd328f08ad12000], // 139
    [0x3cf35976b217dee2, 0x3fd36bf614dcc000], // 140
    [0x3d0b2eeb0e59e070, 0x3fd3af11a079a600], // 141
    [0x3cf0819827b33885, 0x3fd3f2438d754b00], // 142
    [0x3d0c7b3c9413fc6a, 0x3fd4358c3ca03200], // 143
    [0x3d0964bdc3d70397, 0x3fd478ec0fd41900], // 144
    [0x3d0cdb89176122e8, 0x3fd4bc6369fa4000], // 145
    [0x3d0839f45b8f25a7, 0x3fd4fff2af11e200], // 146
    [0x3cc38d46d310526b, 0x3fd5439a4436d000], // 147
    [0x3cec5facdc0a9fc5, 0x3fd5875a8fa83500], // 148
    [0x3d082a056b89a1c8, 0x3fd5cb33f8cf8a00], // 149
    [0x3ce62869782b2a8d, 0x3fd60f26e847b100], // 150
    [0x3d04796c0a72c7ca, 0x3fd65333c7e43a00], // 151
    [0x3cfd6a870a03117a, 0x3fd6975b02b8e300], // 152
    [0x3ce3d9e5812799b1, 0x3fd6db9d05213b00], // 153
    [0x3d08c6939f846807, 0x3fd71ffa3cc87f00], // 154
    [0x3ce39c133af39b89, 0x3fd7647318b1ad00], // 155
    [0x3d0db033ec17a30e, 0x3fd7a908093fc100], // 156
    [0x3ce442bb6d219c7b, 0x3fd7edb9803e3c00], // 157
    [0x3d003298182a0b9a, 0x3fd83287f0e9cf00], // 158
    [0x3d0f476f79cd4d63, 0x3fd87773cff95600], // 159
    [0x3cf5248bb71e4b38, 0x3fd8bc7d93a70400], // 160
    [0x3cf4ffa86c03e743, 0x3fd901a5b3b9cf00], // 161
    [0x3cdba4032d968ff1, 0x3fd946eca98f2700], // 162
    [0x3cbe7be1ab8c95c9, 0x3fd98c52f024e800], // 163
    [0x3cfdcb4229a88adc, 0x3fd9d1d904239800], // 164
    [0x3cd818c3c1ebfac7, 0x3fda177f63e8ef00], // 165
    [0x3cf7d17efea8203f, 0x3fda5d468f92a500], // 166
    [0x3d0ee2b526a79dbc, 0x3fdaa32f09099800], // 167
    [0x3cc545c014943439, 0x3fdae939540d3f00], // 168
    [0x3cfd298f0bcb2a39, 0x3fdb2f65f63f6c00], // 169
    [0x3d0f46219c3642fd, 0x3fdb75b577307500], // 170
    [0x3d0b9096eb994d88, 0x3fdbbc28606bab00], // 171
    [0x3ce912ecb3ffdc91, 0x3fdc02bf3d843400], // 172
    [0x3d03c102038b704b, 0x3fdc497a9c224700], // 173
    [0x3ce23577547dbe24, 0x3fdc905b0c10d400], // 174
    [0x3cbf6217ae80aadf, 0x3fdcd7611f4b8a00], // 175
    [0x3d08b9370246611b, 0x3fdd1e8d6a0d5600], // 176
    [0x3cfe799a1caf5494, 0x3fdd65e082df5200], // 177
    [0x3ce065d0ec2d5d4d, 0x3fddad5b02a82400], // 178
    [0x3cf9f7adea9ee333, 0x3fddf4fd84bbe100], // 179
    [0x3d0de46f5a51f49c, 0x3fde3cc8a6ec6e00], // 180
    [0x3cdc78f6490a2d31, 0x3fde84bd099a6600], // 181
    [0x3d0ba75db6ad4905, 0x3fdeccdb4fc68500], // 182
    [0x3d0e881e78dacbed, 0x3fdf15241f23b300], // 183
    [0x3d06d8897a7f6e8b, 0x3fdf5d9820299400], // 184
    [0x3cf8c568e69982a7, 0x3fdfa637fe27bf00], // 185
    [0x3d010f1ac0685d79, 0x3fdfef0467599500], // 186
    [0x3cf20e63d153de0b, 0x3fe01bff067d6200], // 187
    [0x3d01abda24b59907, 0x3fe04092d1ae3b00], // 188
    [0x3d1b2991b98e444f, 0x3fe0653df0fd9f00], // 189
    [0x3cfb5c445db0513a, 0x3fe08a00c1cae300], // 190
    [0x3ced5941cd486e46, 0x3fe0aedba3221c00], // 191
    [0x3d19b6701042299b, 0x3fe0e651e8522900], // 192
    [0x3cf1f1be7bb3f0ec, 0x3fe1309cbf4cdb00], // 193
    [0x3ce81dd055b792f1, 0x3fe17b4ee1641300], // 194
    [0x3d0b31d463d6bbb9, 0x3fe1c66b9ffd6600], // 195
    [0x3d14290186b44f69, 0x3fe211f66db3a500], // 196
    [0x3d0052f0c90a1e8d, 0x3fe25df2e05b6c00], // 197
    [0x3d10699cc524cc1e, 0x3fe2aa64b32f7700], // 198
    [0x3d1b8343b81df8be, 0x3fe2f74fc9289a00], // 199
    [0x3d1bdde431bc8275, 0x3fe344b82f859a00], // 200
    [0x3d1d2619ba201205, 0x3fe392a22087b700], // 201
    [0x3cf15d0b3143fe69, 0x3fe3e11206694500], // 202
    [0x3cf1d367143da658, 0x3fe4300c7e945000], // 203
    [0x3d0de5f1e93b0759, 0x3fe47f965d201d00], // 204
    [0x3cfefd6da9a38a1e, 0x3fe4cfb4b09d1a00], // 205
    [0x3ce2798b38e54193, 0x3fe5206cc637e000], // 206
    [0x3d1cebe5ce361653, 0x3fe571c42e3d0b00], // 207
    [0x3d06db04d58c602b, 0x3fe5c3c0c108f900], // 208
    [0x3d10352125d3c782, 0x3fe61668a46ffa00], // 209
    [0x3d1ce7c4b779da7f, 0x3fe669c251ad6900], // 210
    [0x3d19c6b29937c576, 0x3fe6bdd49bea0500], // 211
    [0x3d123bb8de524464, 0x3fe712a6b76c6e00], // 212
    [0x3d14ed23742a1962, 0x3fe7684041897800], // 213
    [0x3d04ded0bb1b85f5, 0x3fe7bea9496d5a00], // 214
    [0x3d1440eb506ff105, 0x3fe815ea59dab000], // 215
    [0x3d0903bfcfb17fe2, 0x3fe86e0c84010700], // 216
    [0x3d1bb33f98d0cd75, 0x3fe8c7196b922500], // 217
    [0x3d105e272d4d455a, 0x3fe9211b54441000], // 218
    [0x3d1a3d6ecac25a3a, 0x3fe97c1d30f5b700], // 219
    [0x3d1fa7fcde4fd007, 0x3fe9d82ab4b5fd00], // 220
    [0x3d147bd207497d6d, 0x3fea355065f87f00], // 221
    [0x3d140f495a7dfa2b, 0x3fea939bb451e200], // 222
    [0x3cf73b63186f5ef5, 0x3feaf31b11270200], // 223
    [0x3cf1e1722fb4750a, 0x3feb53de0bcffc00], // 224
    [0x3d0c31ddd0ca6985, 0x3febb5f571cb0500], // 225
    [0x3d1e11d7b6ab862f, 0x3fec197373bc7b00], // 226
    [0x3d0d7f17f5265656, 0x3fec7e6bd023da00], // 227
    [0x3cfd1a09a008995b, 0x3fece4f404e29b00], // 228
    [0x3cd5a7fb0d1d4276, 0x3fed4d2388f63600], // 229
    [0x3cdbda21f01193f2, 0x3fedb714101e0a00], // 230
    [0x3ce7231177f85f71, 0x3fee22e1da97bb00], // 231
    [0x3cf9e452a299b1d2, 0x3fee90ac13b18200], // 232
    [0x3d1e431faf3d196b, 0x3fef009542b71200], // 233
    [0x3cf4ca9c5fe08810, 0x3fef72c3d2c57500], // 234
    [0x3d1aa68f14d779f2, 0x3fefe762b7774400], // 235
    [0x3ced6e11782c28fc, 0x3ff02f511b223c00], // 236
    [0x3d2971fc71c1ffc0, 0x3ff06c5c6f8ce900], // 237
    [0x3d2654e5a5fb29de, 0x3ff0aaf261370000], // 238
    [0x3cf8054c158610de, 0x3ff0eb367c3fd600], // 239
];

// Constants (RVA 0x36a90..0x36b27).
const PIO2_LO: f64 = f64::from_bits(0x3c91a62633145c07); //    (0x36a90)
const PIO2_HI: f64 = f64::from_bits(0x3ff921fb54442d18); //    (0x36a98)
const MASK_27: u64 = 0xfffffffff8000000; //                   (0x36aa0)
const MASK_ABS_22: u64 = 0x7fffffc000000000; //               (0x36ab0)
const C3: f64 = f64::from_bits(0x3fc5555555555555); // 1/6    (0x36ab8)
const C5: f64 = f64::from_bits(0x3fb3333333333333); // 3/40   (0x36ac0)
const C7: f64 = f64::from_bits(0x3fa6db6db6db6db7); // 5/112  (0x36ac8)
// Packed pairs `[lo, hi]` of the odd series (RVA 0x36ad0..0x36b0f).
const P_AD: [f64; 2] = [
    f64::from_bits(0x3f96e8ba2e8ba2e9),
    f64::from_bits(0x3fb3333333333333),
];
const P_AE: [f64; 2] = [
    f64::from_bits(0x3f9f1c71c71c71c7),
    f64::from_bits(0x3fc5555555555555),
];
const P_AF: [f64; 2] = [
    f64::from_bits(0x3f91c4ec4ec4ec4f),
    f64::from_bits(0x3fa6db6db6db6db7),
];
const P_B0: [f64; 2] = [
    f64::from_bits(0x3f87a87878787224),
    f64::from_bits(0x3f8c99999999999a),
];
pub(crate) const MASK_18: u64 = 0xffffc00000000000; //                   (0x36b18)
const ONE: f64 = f64::from_bits(0x3ff0000000000000); //       (0x36b20)
/// `0x0000200000000000`, made by `pinsrw xmm5, 0x2000, 2`: the bit below [`MASK_18`].
pub(crate) const HALF_18: u64 = 0x0000_2000_0000_0000;

const SIGN: u64 = 0x8000_0000_0000_0000;

#[inline]
fn f(b: u64) -> f64 {
    f64::from_bits(b)
}

/// `asin` exactly as `msvcr110.dll!_libm_sse2_asin_precise` (RVA 0x34e40) computes it, under
/// the default MXCSR and x87 control word.
pub fn asin(x: f64) -> f64 {
    let bits = x.to_bits();
    let k = ((bits >> 44) as u32) & 0x7ffff;
    let a = k.wrapping_sub(0x3fb00);
    if a < 0x3bb {
        return mid(x, (bits >> 44) as u32);
    }
    let b = a.wrapping_sub(0x3bb);
    if b < 0x43 {
        return upper(x);
    }
    let c = b.wrapping_add(0x3bbb);
    if c < 0x3800 {
        return small(x);
    }
    let d = c.wrapping_sub(0x3bfe);
    if d < 2 {
        return near_one(x);
    }
    special(x, d.wrapping_add(0x3fefe))
}

/// RVA 0x34ec8: `0.0625 <= |x| < 0.8652`.
fn mid(x: f64, edx: u32) -> f64 {
    let x2 = x * x; // xmm1
    let s = (ONE - x2).sqrt(); // xmm3 = sqrt(1 - x^2)
    let c = f(MASK_18 & x.to_bits() | HALF_18); // xmm2 = trunc(x) | half-unit: the table point
    let i = ((((edx & 0xffff) & 0xffff_fffc).wrapping_sub(0xfb00)) >> 2) as usize;
    let sc = f(SQRT_TABLE[i]); // xmm1
    let t = ASIN_TABLE[i]; // xmm4 = [lo, hi]
    let x7 = x + c; // xmm7
    let x0 = x - c; // xmm0
    let x0 = x0 * x7; // x^2 - c^2
    let x6 = x * sc; // xmm6
    let x3 = s * c; // xmm3
    let x1 = x6; // xmm1
    let x6 = x6 + x3;
    let delta = x0 / x6; // xmm0
    let d = x1 - x3; // xmm1
    let sign = c.to_bits() & SIGN; // xmm2, both lanes
    let d2 = d * d; // xmm1
    let d3 = d * d2; // xmm3
    let q7 = C7 * d2; // xmm7
    let t_lo = f(t[0] ^ sign); // xmm4 ^= sign
    let t_hi = f(t[1] ^ sign);
    let q3 = C3 * d3; // xmm5
    let d5 = d3 * d2; // xmm3
    let q5 = C5 + q7; // xmm6
    let q5 = q5 * d5;
    let q3 = q3 + t_lo; // xmm5
    let q5 = q5 + q3; // xmm6
    let r = delta + q5; // xmm0
    r + t_hi
}

/// RVA 0x34f97: `0.8652 <= |x| < 0.9961`.
fn upper(x: f64) -> f64 {
    let x2 = x * x; // xmm1
    let s = (ONE - x2).sqrt(); // xmm3
    let sign_mask = x.to_bits() & SIGN; // pmovmskb bit 7, later `(eax & 0x80) << 8`
    let ax = f(x.to_bits() & !SIGN); // xmm0 = |x|
    let xh = f(x.to_bits() & MASK_ABS_22); // xmm7 = trunc(|x|)
    let x1 = ax - xh; // xmm1
    let xh2 = xh * xh; // xmm7
    let x0 = ax + xh; // xmm0
    let x4 = ONE - xh2; // xmm4
    let x0 = x0 * x1; // x^2 - xh^2
    let sh = f(MASK_18 & s.to_bits() | HALF_18); // xmm2: table point for s
    let j = ((((s.to_bits() << 2) >> 48) as u32).wrapping_sub(0xfec0)) as usize; // edx / 2
    let x7 = s * f(SQRT_TABLE[j]); // xmm7
    let x6 = xh * sh; // xmm6
    let t = ASIN_TABLE[j];
    let x1 = x1 * sh; // xmm1
    let sh2 = sh * sh; // xmm2
    let x6 = x6 - x7;
    let e = x6 + x1; // xmm6
    let x4 = x4 - sh2;
    let x7 = x7 + x7;
    let x4 = x4 - x0;
    let x7 = x7 + e;
    let corr = x4 / x7; // xmm4
    let c_lo = PIO2_LO - f(t[0]); // xmm3 = [pio2_lo, pio2_hi] - table
    let c_hi = PIO2_HI - f(t[1]);
    let e2 = e * e; // xmm6
    let q7 = C7 * e2; // xmm0
    let e3 = e * e2; // xmm1
    let q3 = C3 * e3; // xmm5
    let e5 = e3 * e2; // xmm1
    let q = q7 + C5; // xmm0
    let q = q * e5;
    let q3 = q3 + c_lo; // xmm5
    let q = q + q3; // xmm0
    let q = q - corr;
    let q = q + c_hi;
    f(q.to_bits() | sign_mask)
}

/// RVA 0x350af: `2^-60 <= |x| < 0.0625`.
fn small(x: f64) -> f64 {
    let x2 = x * x; // xmm7 = [x^2, x^2]
    let x3 = x * x2; // xmm1 = [x^3, x^3]
    let p_lo = P_AD[0] * x2; // xmm6
    let p_hi = P_AD[1] * x2;
    let x4 = x2 * x2; // xmm7
    let x6 = x3 * x3; // xmm1.lo
    let p_lo = p_lo + P_AE[0];
    let p_hi = p_hi + P_AE[1];
    let q_lo = P_AF[0] * x4; // xmm4
    let q_hi = P_AF[1] * x4;
    let x9 = x6 * x3; // xmm1.lo (xmm3.lo = x^3)
    let p_lo = p_lo + q_lo;
    let p_hi = p_hi + q_hi;
    let r_lo = x9 * p_lo; // xmm1 = [x^9, x^3] * p
    let r_hi = x3 * p_hi;
    let r = r_lo + r_hi;
    x + r
}

/// RVA 0x3511a: `0.9961 <= |x| < 1`.
fn near_one(x: f64) -> f64 {
    let x2 = x * x; // xmm1
    let s = (ONE - x2).sqrt(); // xmm3.lo, xmm5 = [s, s]
    let sign = x.to_bits() & SIGN; // eax = word 3, `& 0x8000`
    let xh = f(x.to_bits() & MASK_27); // xmm7
    let sh = f(s.to_bits() & MASK_27); // xmm3
    let x1 = xh; // xmm1
    let x0 = x - xh; // xmm0
    let xh2 = xh * xh; // xmm7
    let x1 = x1 + x1; // 2 xh
    let x1 = x1 * x0; // 2 xh (x - xh)
    let x4 = ONE - xh2; // xmm4
    let x6 = sh; // xmm6
    let sh2 = sh * sh; // xmm3
    let x0 = x0 * x0; // (x - xh)^2
    let x4 = x4 - x1;
    let x6 = x6 - s; // sh - s
    let s2 = s + s; // xmm5.lo
    let x4 = x4 - sh2;
    let x4 = x4 - x0;
    let x5 = s2 + x6; // xmm5.lo
    let x5 = x5 * x6;
    let x4 = x4 + x5;
    let corr = x4 / s2; // xmm4 (xmm3.lo = 2s)
    let ss = s * s; // xmm7 = [s^2, s^2]
    let p_lo = P_AD[0] * ss; // xmm2
    let p_hi = P_AD[1] * ss;
    let s3 = s * ss; // xmm6 = [s^3, s^3]
    let b_lo = P_B0[0] * ss; // xmm1.lo (xmm1.hi = P_B0[1])
    let s4 = ss * ss; // xmm7
    let p_lo = P_AE[0] + p_lo; // xmm5
    let p_hi = P_AE[1] + p_hi;
    let s6 = s3 * s3; // xmm6.lo (xmm2 = [s^3, s^3])
    let q_lo = s4 * P_AF[0]; // xmm7
    let q_hi = s4 * P_AF[1];
    let s9 = s3 * s6; // xmm2.lo
    let q_lo = q_lo + p_lo;
    let q_hi = q_hi + p_hi;
    let s15 = s6 * s9; // xmm6.lo
    let q_lo = q_lo * s9; // xmm7 = [s^9, s^3] * q
    let q_hi = q_hi * s3;
    let b = b_lo + P_B0[1]; // xmm1.lo
    let b = b * s15;
    let q = q_lo + q_hi; // xmm7.lo
    let t3 = s - PIO2_HI; // xmm3.lo
    let r = PIO2_LO + b; // xmm0.lo
    let t6 = PIO2_HI + t3; // xmm6.lo
    let q = q + corr; // xmm7.lo
    let t2 = s - t6; // xmm2.lo
    let r = r - q;
    let r = r - t2;
    let r = r - t3;
    f(r.to_bits() | sign)
}

/// RVA 0x3525c: tiny, `|x| >= 1`, infinities and NaNs. `k` is `(bits >> 44) & 0x7ffff`.
fn special(x: f64, k: u32) -> f64 {
    let bits = x.to_bits();
    if k < 0x3ff00 {
        // |x| < 2^-60 (RVA 0x35339): x itself (the extra operations only raise flags).
        return x;
    }
    let ahi = ((bits >> 32) as u32) & 0x7fff_ffff;
    let lo = bits as u32;
    if (0x3ff0_0000u32.wrapping_sub(ahi) | lo) == 0 {
        // |x| == 1 (RVA 0x352fd).
        return f((PIO2_HI + PIO2_LO).to_bits() | (bits & SIGN));
    }
    if bits & !SIGN > 0x7ff0_0000_0000_0000 {
        // NaN: `x + 0.0` (quiets it), error 1009, returned unchanged.
        return f(bits | 0x0008_0000_0000_0000);
    }
    // |x| > 1: `0 * inf`, the default NaN, error 61.
    f(0xfff8_0000_0000_0000)
}
