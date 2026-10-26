// SPDX-License-Identifier: Apache-2.0
#import <Cocoa/Cocoa.h>
#include "../realqa.h"
#include <iostream>
using namespace lane::desktop;
void Check(bool ok,const char* message) { if(!ok)throw std::runtime_error(message); }
int main() {
 @autoreleasepool {
  [NSApplication sharedApplication];
  auto host=CreateCaptureHost(std::make_shared<Events>());
  NSBitmapImageRep* source=[[NSBitmapImageRep alloc] initWithBitmapDataPlanes:nil pixelsWide:20 pixelsHigh:20 bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
  for(int y=0;y<20;y++)for(int x=0;x<20;x++)[source setColor:[NSColor colorWithDeviceRed:(y<10?1:0) green:(y<10?1:0) blue:(y<10?1:0) alpha:1] atX:x y:y];
  auto png=std::string([[source representationUsingType:NSBitmapImageFileTypePNG properties:@{}] base64EncodedStringWithOptions:0].UTF8String);
  auto call=[&](CaptureJSON request){return CaptureJSON::parse((*host)("capture",{request.dump()}));};
  try {
    auto result=call({{"op","render"},{"png",png},{"layers",CaptureJSON::array({{{"type","cover"},{"x",0},{"y",0},{"width",5},{"height",5},{"color","#FF0000"}}})}});
    Check(result["ok"],"render failed");Check(result["value"]["width"]==20,"width mismatch");
    auto encoded=result["value"]["png"].get<std::string>();
    NSBitmapImageRep* decoded=[NSBitmapImageRep imageRepWithData:[[NSData alloc] initWithBase64EncodedString:[NSString stringWithUTF8String:encoded.c_str()] options:0]];
    // Inspect encoded pixel samples, independent of the host display profile.
    NSUInteger top[4]={0}, bottom[4]={0};
    [decoded getPixel:top atX:2 y:2]; [decoded getPixel:bottom atX:2 y:18];
    Check(top[0]==255&&top[1]==0&&top[2]==0&&top[3]==255,"cover must render opaque red at top-left");
    Check(bottom[0]==0&&bottom[1]==0&&bottom[2]==0,"original orientation changed");
    auto crop=call({{"op","render"},{"png",png},{"layers",CaptureJSON::array()},{"crop",{{"x",0},{"y",0},{"width",8},{"height",7}}}});
    Check(crop["ok"]&&crop["value"]["width"]==8&&crop["value"]["height"]==7,"crop failed");
    auto invalid=call({{"op","render"},{"png",png},{"layers",CaptureJSON::array()},{"crop",{{"x",19},{"y",0},{"width",8},{"height",7}}}});
    Check(!invalid["ok"],"invalid crop accepted");
    auto tools=CaptureJSON::array();
    for(auto type:{"draw","arrow","rectangle","text","blur","cover"})tools.push_back({{"type",type},{"x",2},{"y",2},{"width",10},{"height",10},{"text","QA"},{"color","#0000FF"}});
    Check(call({{"op","render"},{"png",png},{"layers",tools}})["ok"],"editor tool failed");
    Check(!call({{"op","render"},{"png","bad data"},{"layers",CaptureJSON::array()}})["ok"],"malformed image accepted");
    std::cout<<"Lane RealQA memory renderer tests passed\n";return 0;
  }catch(const std::exception& error){std::cerr<<error.what()<<'\n';return 1;}
 }
}
